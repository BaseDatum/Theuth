//! The `decide` / `decide_many` / `decide_label` runtime functions (design doc 92): the
//! classifier as a tool the reasoning model programs from a Monty cell — many similar items,
//! fixed labels, a probability per label back; the model reads only the unsure tail.
//!
//! `labels` are what the classifier reads; `ids` are the keys it answers with and are never
//! rendered — every checkpoint sees positional ids (`0..n`) plus `none` (57 §0.4). Rows are
//! sorted by rendered length, sent as one [`AskMany`] per `decision.tool.batch` rows on the
//! family's (or the call's) checkpoint, and come back in input order; a per-row failure is an
//! `error` on that row, the call fails only for malformed arguments or a cap. When the call
//! returns, the checkpoint is released unless another live family runs on it (57 §2.1).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::engine::{AskMany, NONE_ID, Release};
use crate::gate::Mode;
use crate::question::{Ask, Candidate, Decision, Disposition, LogId, Question, Reason};
use crate::training::{Amend, Amendment, Asked, Example, ExampleId, Row, Store, Verdict};
use crate::{DecisionConfig, Engine, TOOL_FAMILY, actor_name};

/// Most `log_id → task` entries kept for [`DecideLabelTool`]; the oldest go first (uuid v7).
// ponytail: in-memory only — a daemon restart forgets which rows were captured; persist the map
// beside the training store if labelling across restarts turns out to matter.
const CAPTURED_MAX: usize = 50_000;

/// `tool` or `tool.<task>` (92 §2): the families the `decide` tool asks under. They have no
/// tier 2 ([`crate::escalate`] refuses them).
pub fn is_tool_family(family: &str) -> bool {
    family == TOOL_FAMILY
        || family
            .strip_prefix(TOOL_FAMILY)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// One question as the model writes it (92 §1.1). `state` is the item — discriminating text
/// first, it is right-truncated to the model window (~512 subwords) and capped at
/// `decision.tool.max_state_bytes`.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Q {
    /// Pick one label, or `none` when none fits.
    Choice {
        /// What to decide, e.g. "Which folder does this email belong in?".
        instructions: String,
        /// Short descriptive phrases the classifier reads ("invoices and receipts"), not codes.
        labels: Vec<String>,
        /// Keys answered in `choice` / `probs`, one per label; default = the labels. Never
        /// shown to the classifier. `none` is reserved.
        #[serde(default)]
        ids: Option<Vec<String>>,
        /// The item.
        state: String,
    },
    /// Does `statement` hold for `state`? `choice` is `"true"` or `"false"`.
    Bool {
        /// The statement.
        statement: String,
        /// The item.
        state: String,
    },
    /// An ordinal level; `choice` is the level index as a string (`"0"`, `"1"`, …).
    Score {
        /// What to rate.
        instructions: String,
        /// One criterion per level, index = level.
        levels: Vec<String>,
        /// The item.
        state: String,
    },
}

impl Q {
    fn state(&self) -> &str {
        match self {
            Q::Choice { state, .. } | Q::Bool { state, .. } | Q::Score { state, .. } => state,
        }
    }

    /// Bytes the classifier reads: a stand-in for the token count the batches are sorted by.
    fn rendered_len(&self) -> usize {
        match self {
            Q::Choice {
                instructions,
                labels,
                state,
                ..
            } => instructions.len() + labels.iter().map(String::len).sum::<usize>() + state.len(),
            Q::Bool { statement, state } => statement.len() + state.len(),
            Q::Score {
                instructions,
                levels,
                state,
            } => instructions.len() + levels.iter().map(String::len).sum::<usize>() + state.len(),
        }
    }

    /// The engine question plus the key each engine label maps back to (`none` → `none`),
    /// or why this row cannot be asked.
    fn prepare(&self) -> Result<(Question, Vec<String>), String> {
        match self {
            Q::Choice {
                instructions,
                labels,
                ids,
                state,
            } => {
                if labels.is_empty() {
                    return Err("no labels".into());
                }
                let keys = ids.as_deref().unwrap_or(labels);
                if keys.len() != labels.len() {
                    return Err(format!(
                        "{} ids for {} labels — one id per label",
                        keys.len(),
                        labels.len()
                    ));
                }
                if let Some(k) = keys.iter().find(|k| k.as_str() == NONE_ID) {
                    return Err(format!("id `{k}` is reserved for the none control"));
                }
                let mut seen = std::collections::HashSet::new();
                if let Some(k) = keys.iter().find(|k| !seen.insert(k.as_str())) {
                    return Err(format!("duplicate id `{k}`"));
                }
                let candidates = labels
                    .iter()
                    .enumerate()
                    .map(|(i, text)| Candidate {
                        id: i.to_string(),
                        text: text.clone(),
                    })
                    .collect();
                Ok((
                    Question::Choice {
                        instructions: instructions.clone(),
                        candidates,
                        state: state.clone(),
                    },
                    keys.to_vec(),
                ))
            }
            Q::Bool { statement, state } => Ok((
                Question::Bool {
                    statement: statement.clone(),
                    state: state.clone(),
                },
                vec!["false".to_owned(), "true".to_owned()],
            )),
            Q::Score {
                instructions,
                levels,
                state,
            } => {
                if levels.is_empty() {
                    return Err("no levels".into());
                }
                Ok((
                    Question::Score {
                        instructions: instructions.clone(),
                        levels: levels.clone(),
                        state: state.clone(),
                    },
                    (0..levels.len()).map(|i| i.to_string()).collect(),
                ))
            }
        }
    }

    /// The question as the training row records it: the model's own keys as candidate ids.
    fn as_asked(&self) -> Question {
        match self {
            Q::Choice {
                instructions,
                labels,
                ids,
                state,
            } => Question::Choice {
                instructions: instructions.clone(),
                candidates: ids
                    .as_deref()
                    .unwrap_or(labels)
                    .iter()
                    .zip(labels)
                    .map(|(id, text)| Candidate {
                        id: id.clone(),
                        text: text.clone(),
                    })
                    .collect(),
                state: state.clone(),
            },
            Q::Bool { statement, state } => Question::Bool {
                statement: statement.clone(),
                state: state.clone(),
            },
            Q::Score {
                instructions,
                levels,
                state,
            } => Question::Score {
                instructions: instructions.clone(),
                levels: levels.clone(),
                state: state.clone(),
            },
        }
    }
}

/// One answer (92 §1.1), in input order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RowOut {
    /// The winning key, `none`, or `null` on `error`.
    pub choice: Option<String>,
    /// Probability per key (`none` included for a choice); empty on `error`.
    pub probs: BTreeMap<String, f32>,
    /// Top-1 − top-2 style margin from the engine (`Scores::confidence`); a raw margin until
    /// `calibrated`.
    pub confidence: f32,
    /// A fitted temperature was applied (92 §5).
    pub calibrated: bool,
    /// The state was cut to the model window; the answer stands but saw less.
    pub truncated: bool,
    /// The decision-log id; `decide_label` takes it.
    pub log_id: String,
    /// Why this row has no answer; the other rows are unaffected.
    pub error: Option<String>,
}

impl RowOut {
    fn error(msg: String) -> Self {
        RowOut {
            choice: None,
            probs: BTreeMap::new(),
            confidence: 0.0,
            calibrated: false,
            truncated: false,
            log_id: String::new(),
            error: Some(msg),
        }
    }

    /// The engine's decision mapped back onto the caller's keys.
    fn from_decision(d: &Decision, keys: &[String], budget: Duration) -> Self {
        let disposition = match &d.disposition {
            Disposition::Shadow(inner) => &inner.disposition,
            other => other,
        };
        let error = match disposition {
            Disposition::Accept => None,
            Disposition::Escalate(reason) => Some(match reason {
                Reason::LowTop1 | Reason::LowMargin => {
                    unreachable!("tool gates have no thresholds")
                }
                Reason::HardError(s) => s.clone(),
                Reason::Truncated => {
                    "truncated: the labels or the state did not fit the model window — shorten them"
                        .to_owned()
                }
                Reason::Timeout => format!(
                    "budget: the batch did not finish in {} ms — raise budget_ms or pass fewer rows",
                    budget.as_millis()
                ),
                Reason::Disabled => "the decision family is off".to_owned(),
                Reason::ModelMissing => {
                    "checkpoint not installed — scripts/fetch-models.sh downloads it".to_owned()
                }
            }),
            Disposition::Shadow(_) => unreachable!("unwrapped above"),
        };
        let key = |label: &str| -> String {
            if label == NONE_ID {
                return NONE_ID.to_owned();
            }
            label
                .parse::<usize>()
                .ok()
                .and_then(|i| keys.get(i))
                .cloned()
                .unwrap_or_else(|| label.to_owned())
        };
        let (choice, probs) = if error.is_some() {
            (None, BTreeMap::new())
        } else {
            (
                Some(key(&d.choice)),
                d.probabilities.iter().map(|(l, p)| (key(l), *p)).collect(),
            )
        };
        RowOut {
            choice,
            probs,
            confidence: if error.is_some() { 0.0 } else { d.confidence },
            calibrated: d.calibrated,
            truncated: error.is_none() && d.truncated,
            log_id: d.log_id.to_string(),
            error,
        }
    }
}

/// `decide_many` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DecideManyArgs {
    /// The questions, answered in this order; at most `decision.tool.max_rows` (2,000).
    pub questions: Vec<Q>,
    /// Checkpoint: `gliner` (default; invented labels) or `default` (laya, 3–5× faster, for
    /// its fine-tuned families only).
    #[serde(default)]
    pub checkpoint: Option<String>,
    /// `tool` (default) or a configured `tool.<task>` sub-family with its own calibration.
    #[serde(default)]
    pub family: Option<String>,
    /// Record every row as a training example under `tool.<task>` so `decide_label` can label
    /// it later; e.g. `"mail"`. Only when the user asked for a sorter that should learn.
    #[serde(default)]
    pub capture: Option<String>,
    /// Wall clock per batch of `decision.tool.batch` rows; default the family's (2,000 ms).
    #[serde(default)]
    pub budget_ms: Option<u64>,
}

/// `decide` arguments: one question.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DecideArgs {
    /// The question.
    pub question: Q,
    /// See `decide_many`.
    #[serde(default)]
    pub checkpoint: Option<String>,
    /// See `decide_many`.
    #[serde(default)]
    pub family: Option<String>,
    /// See `decide_many`.
    #[serde(default)]
    pub capture: Option<String>,
    /// See `decide_many`.
    #[serde(default)]
    pub budget_ms: Option<u64>,
}

/// `decide_label` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DecideLabelArgs {
    /// A row's `log_id` from a `decide_many` call made with `capture`.
    pub log_id: String,
    /// The right key for that row (`none` when nothing fits).
    pub id: String,
}

/// What the three tools share.
pub struct Decide {
    cfg: DecisionConfig,
    /// `<data_dir>/decisions/training` (the engine's store, same files).
    training: Store,
    /// `log_id → tool.<task>` of the rows a `capture` call wrote, newest-bounded.
    captured: Mutex<BTreeMap<LogId, Arc<str>>>,
}

/// The resolved target of one call.
struct Target {
    family: String,
    checkpoint: String,
    budget: Duration,
    engine: ActorRef<Engine>,
    note: Option<String>,
}

impl Decide {
    /// Over `cfg`, writing captures beside the engine's training store under `decisions_dir`.
    pub fn new(cfg: DecisionConfig, decisions_dir: &std::path::Path) -> Self {
        Decide {
            cfg,
            training: Store::new(decisions_dir.join("training")),
            captured: Mutex::new(BTreeMap::new()),
        }
    }

    /// 92 §2: `family` must be `tool` or `tool.<task>`; an unconfigured sub-family falls back
    /// to `tool` with a note; the checkpoint is the call's or the gate's and must have an engine.
    fn target(
        &self,
        family: Option<&str>,
        checkpoint: Option<&str>,
        budget_ms: Option<u64>,
    ) -> Result<Target, String> {
        let asked = family.unwrap_or(TOOL_FAMILY);
        if !is_tool_family(asked) {
            return Err(format!(
                "family must be `{TOOL_FAMILY}` or `{TOOL_FAMILY}.<task>`, not `{asked}`"
            ));
        }
        let (family, gate, note) = match self.cfg.families.get(asked) {
            Some(g) => (asked.to_owned(), g.clone(), None),
            None => {
                let g =
                    self.cfg.families.get(TOOL_FAMILY).ok_or_else(|| {
                        format!("no `{TOOL_FAMILY}` decision family is configured")
                    })?;
                (
                    TOOL_FAMILY.to_owned(),
                    g.clone(),
                    Some(format!(
                        "family `{asked}` is not configured; answered as `{TOOL_FAMILY}`"
                    )),
                )
            }
        };
        if gate.mode == Mode::Off {
            return Err(format!(
                "decision family `{family}` is off (decision.families.{family}.mode)"
            ));
        }
        let checkpoint = checkpoint.unwrap_or_else(|| gate.checkpoint()).to_owned();
        let engine = ActorRef::<Engine>::lookup(actor_name(&checkpoint).as_str())
            .ok()
            .flatten()
            .ok_or_else(|| {
                format!(
                    "checkpoint `{checkpoint}` has no engine — add it under decision.checkpoints"
                )
            })?;
        Ok(Target {
            budget: budget_ms.map_or(gate.budget, Duration::from_millis),
            family,
            checkpoint,
            engine,
            note,
        })
    }

    /// Whether a live family other than the tool's runs on `checkpoint` (57 §2.1): then the
    /// tool must not release it.
    fn held_by_others(&self, checkpoint: &str) -> bool {
        self.cfg
            .families
            .iter()
            .any(|(k, g)| !is_tool_family(k) && g.mode != Mode::Off && g.checkpoint() == checkpoint)
    }

    /// `capture` → the `tool.<task>` prefix (`"mail"` and `"tool.mail"` both work).
    fn capture_prefix(capture: &str) -> Result<Arc<str>, String> {
        let task = capture
            .strip_prefix(TOOL_FAMILY)
            .and_then(|r| r.strip_prefix('.'))
            .unwrap_or(capture)
            .trim();
        if task.is_empty()
            || !task
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "capture must name a task (letters, digits, `-`, `_`), e.g. \"mail\"; got {capture:?}"
            ));
        }
        Ok(format!("{TOOL_FAMILY}.{task}").into())
    }

    /// The whole call: caps, target, per-row build, length-sorted batches, input order back,
    /// capture, release.
    async fn run(
        &self,
        questions: Vec<Q>,
        checkpoint: Option<&str>,
        family: Option<&str>,
        capture: Option<&str>,
        budget_ms: Option<u64>,
    ) -> Result<(Vec<RowOut>, String), String> {
        let caps = &self.cfg.tool;
        if questions.len() > caps.max_rows {
            return Err(format!(
                "decide_many: {} rows > max_rows {} — loop over slices",
                commas(questions.len()),
                commas(caps.max_rows)
            ));
        }
        let capture = capture.map(Self::capture_prefix).transpose()?;
        let target = self.target(family, checkpoint, budget_ms)?;

        let mut out: Vec<Option<RowOut>> = Vec::with_capacity(questions.len());
        let mut pending: Vec<(usize, Ask, Vec<String>)> = Vec::new();
        for (i, q) in questions.iter().enumerate() {
            let state_len = q.state().len();
            if state_len > caps.max_state_bytes {
                out.push(Some(RowOut::error(format!(
                    "state is {} bytes; cap {} — pass a slice (sender, subject, first lines)",
                    commas(state_len),
                    commas(caps.max_state_bytes)
                ))));
                continue;
            }
            match q.prepare() {
                Ok((question, keys)) => {
                    let mut ask = Ask::new(target.family.as_str(), question);
                    ask.budget = target.budget;
                    pending.push((i, ask, keys));
                    out.push(None);
                }
                Err(e) => out.push(Some(RowOut::error(e))),
            }
        }
        // Shortest first: a batch pads to its longest row, so like lengths share a forward.
        pending.sort_by_key(|(i, _, _)| questions[*i].rendered_len());
        let asked = pending.len();
        let mut errors = 0usize;
        let mut truncated = 0usize;
        for chunk in pending.chunks(caps.batch.max(1)) {
            let asks: Vec<Ask> = chunk.iter().map(|(_, a, _)| a.clone()).collect();
            let decisions = target
                .engine
                .ask(AskMany {
                    asks,
                    checkpoint: Some(target.checkpoint.clone()),
                })
                .await;
            match decisions {
                Ok(ds) => {
                    for ((i, _, keys), d) in chunk.iter().zip(ds) {
                        let row = RowOut::from_decision(&d, keys, target.budget);
                        errors += usize::from(row.error.is_some());
                        truncated += usize::from(row.truncated);
                        if let (Some(prefix), None) = (&capture, &row.error) {
                            self.capture(prefix, &questions[*i], &d, &row);
                        }
                        out[*i] = Some(row);
                    }
                }
                Err(e) => {
                    errors += chunk.len();
                    for (i, _, _) in chunk {
                        out[*i] = Some(RowOut::error(format!("engine: {e}")));
                    }
                }
            }
            // The mailbox interleaves other families' asks between batches; this yields the
            // task too so a browser step is not stalled behind a long sort.
            tokio::task::yield_now().await;
        }
        if !self.held_by_others(&target.checkpoint) {
            let _ = target
                .engine
                .tell(Release {
                    checkpoint: None,
                    reason: "decide_many returned",
                })
                .await;
        }
        let rows: Vec<RowOut> = out.into_iter().flatten().collect();
        let mut summary = format!(
            "{} rows: {} answered, {} errors, {} truncated; family {}, checkpoint {}, batch {}",
            rows.len(),
            asked.saturating_sub(errors),
            rows.len() - asked + errors,
            truncated,
            target.family,
            target.checkpoint,
            caps.batch
        );
        if let Some(prefix) = &capture {
            summary.push_str(&format!("; captured under {prefix}"));
        }
        if let Some(note) = &target.note {
            summary.push_str("; ");
            summary.push_str(note);
        }
        Ok((rows, summary))
    }

    /// One 57 §11 example for a captured row, keyed by its `log_id` so `decide_label` can
    /// amend it without a lookup.
    fn capture(&self, prefix: &Arc<str>, q: &Q, d: &Decision, row: &RowOut) {
        let mut asked = Asked::new(&q.as_asked(), d);
        asked.probabilities = row.probs.clone();
        let mut example = Example::new(
            prefix,
            Mode::Active,
            q.state().to_owned(),
            Vec::new(),
            vec![asked],
            None,
            Verdict::Model {
                reason: "decide_many: the calling model acts on the row".to_owned(),
            },
        );
        example.id = ExampleId(d.log_id.0);
        if let Err(e) = self
            .training
            .append(prefix, &Row::Example(Box::new(example)))
        {
            tracing::warn!("decision training ({prefix}): {e}");
            return;
        }
        let mut captured = self.captured.lock();
        captured.insert(d.log_id, Arc::clone(prefix));
        while captured.len() > CAPTURED_MAX {
            captured.pop_first();
        }
    }

    /// `decide_label`: the label for a captured row, or why not.
    fn label(&self, log_id: &str, id: &str) -> Result<Arc<str>, String> {
        let uuid = uuid::Uuid::parse_str(log_id.trim()).map_err(|e| format!("log_id: {e}"))?;
        let prefix = self
            .captured
            .lock()
            .get(&LogId(uuid))
            .cloned()
            .ok_or_else(|| {
                "row was not captured; pass capture=\"tool.<task>\" to decide_many".to_owned()
            })?;
        let amend = Amend::new(
            ExampleId(uuid),
            Amendment::Label {
                choice: id.to_owned(),
            },
        );
        self.training
            .append(&prefix, &Row::Amend(amend))
            .map_err(|e| format!("decision training ({prefix}): {e}"))?;
        Ok(prefix)
    }
}

fn commas(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The question-dict grammar and the honest numbers, shared by the two asking tools.
const SHAPES: &str = "Question dicts: `{\"type\":\"choice\",\"instructions\":…,\"labels\":[…],\"ids\":[…]?,\"state\":…}` (answer one label or `none`), \
`{\"type\":\"bool\",\"statement\":…,\"state\":…}` (`choice` is \"true\"/\"false\"), \
`{\"type\":\"score\",\"instructions\":…,\"levels\":[…],\"state\":…}` (`choice` is the level index as a string). \
`labels` are what the classifier reads — short descriptive phrases, not codes; `ids` (optional, same length) are the keys returned in `choice`/`probs` and are never shown to it. \
`state` is the item: put the discriminating text first (sender, subject, first lines), ≤ 4 KB; it is right-truncated at ~512 subwords (`truncated: true`). \
Each result row: `{choice, probs, confidence, calibrated, truncated, log_id, error}`. `confidence` is a raw margin until `calibrated` — pick your threshold on a 50-row sample and read the unsure ones yourself; `none` means no label fits. \
Cost: laya ~22 ms/row, GLiNER (default `checkpoint=\"gliner\"`, right for labels you invented) 55→330 ms/row for 72→512 tokens, flat in label count; 1,000 × 150-token rows ≈ 2 min. \
Not for anything needing reasoning, arithmetic or reading long documents. Load the `decide` skill for recipes.";

/// `decide_many`.
pub struct DecideManyTool(pub Arc<Decide>);

#[async_trait::async_trait]
impl Tool for DecideManyTool {
    const NAME: &'static str = "decide_many";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    const CONSUMES_SECRETS: bool = false;
    type Args = DecideManyArgs;

    fn description(&self) -> String {
        format!(
            "Classify many similar items with the local decision model (milliseconds and zero tokens per item): \
             one result row per question, same order. Up to 2,000 questions per call (loop over slices for more); \
             batching is automatic. Pass `capture=\"<task>\"` only when the user wants a sorter that learns, then \
             `decide_label(log_id, id)` what you verified. {SHAPES}"
        )
    }

    async fn call(&self, a: DecideManyArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(
            match self
                .0
                .run(
                    a.questions,
                    a.checkpoint.as_deref(),
                    a.family.as_deref(),
                    a.capture.as_deref(),
                    a.budget_ms,
                )
                .await
            {
                Ok((rows, summary)) => ToolOutput {
                    structured: serde_json::to_value(&rows).ok(),
                    ..ToolOutput::ok(summary)
                },
                Err(e) => ToolOutput::err(e),
            },
        )
    }
}

/// `decide`: one question.
pub struct DecideTool(pub Arc<Decide>);

#[async_trait::async_trait]
impl Tool for DecideTool {
    const NAME: &'static str = "decide";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = DecideArgs;

    fn description(&self) -> String {
        format!(
            "Ask the local decision model one typed question and get a probability per label back; \
             `decide_many` does the same for a list — prefer it for more than a handful of items. {SHAPES}"
        )
    }

    async fn call(&self, a: DecideArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(
            match self
                .0
                .run(
                    vec![a.question],
                    a.checkpoint.as_deref(),
                    a.family.as_deref(),
                    a.capture.as_deref(),
                    a.budget_ms,
                )
                .await
            {
                Ok((mut rows, summary)) => ToolOutput {
                    structured: serde_json::to_value(rows.remove(0)).ok(),
                    ..ToolOutput::ok(summary)
                },
                Err(e) => ToolOutput::err(e),
            },
        )
    }
}

/// `decide_label`.
pub struct DecideLabelTool(pub Arc<Decide>);

#[async_trait::async_trait]
impl Tool for DecideLabelTool {
    const NAME: &'static str = "decide_label";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = DecideLabelArgs;

    fn description(&self) -> String {
        "Record the right answer for a row `decide_many` returned under `capture`: a training example \
         for that task's sorter (57 §11). `id` is one of the row's keys or `none`. Rows from calls without \
         `capture` cannot be labelled."
            .to_owned()
    }

    async fn call(&self, a: DecideLabelArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(match self.0.label(&a.log_id, &a.id) {
            Ok(prefix) => ToolOutput {
                structured: Some(serde_json::json!({ "ok": true, "family": &*prefix })),
                ..ToolOutput::ok(format!("labelled {} as {} under {prefix}", a.log_id, a.id))
            },
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// Register the three tools on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>, decide: Arc<Decide>) {
    let _ = reg
        .ask(RegisterTool(Arc::new(DecideManyTool(Arc::clone(&decide)))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(DecideTool(Arc::clone(&decide)))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(DecideLabelTool(decide))))
        .await;
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use kameo::actor::Spawn;

    use super::*;
    use crate::engine::{Loader, ModelSha, Oracle, Row as EngineRow, Scores, model_files};
    use crate::log::Log;
    use crate::training;
    use crate::{Checkpoint, Kind, LoadedNow};

    #[test]
    fn tool_family_predicate() {
        assert!(is_tool_family("tool"));
        assert!(is_tool_family("tool.mail"));
        assert!(!is_tool_family("tools"));
        assert!(!is_tool_family("toolbox.x"));
        assert!(!is_tool_family("meeting.kind"));
    }

    #[test]
    fn commas_groups_thousands() {
        assert_eq!(commas(5), "5");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(2431), "2,431");
        assert_eq!(commas(1_000_000), "1,000,000");
    }

    /// A row that is not laya's: the option texts and the state, plus the cut flags.
    struct FakeRow {
        ids: Vec<i64>,
        options: Vec<String>,
        state: String,
        cut: bool,
        lost: bool,
    }

    impl EngineRow for FakeRow {
        fn truncated(&self) -> bool {
            self.cut || self.lost
        }
        fn state_cut(&self) -> bool {
            self.cut
        }
        fn markers_lost(&self) -> bool {
            self.lost
        }
        fn ids(&self) -> &[i64] {
            &self.ids
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// What one fake forward saw: the batch's option texts (so a test can prove the caller's
    /// ids never reach the model) and a tag per row.
    #[derive(Default)]
    struct Seen {
        batches: Vec<Vec<String>>,
        options: Vec<Vec<String>>,
    }

    /// Scores the option whose text occurs in the state at 0.8 (the rest share 0.2), uniform
    /// when none does; a state containing `CUT` is a cut row, `LOST` a lost-marker row. Bool
    /// options are `[no, yes]` in the engine's `[false, true]` order.
    struct FakeOracle {
        delay: Duration,
        seen: Arc<Mutex<Seen>>,
    }

    impl Oracle for FakeOracle {
        fn model_sha(&self) -> ModelSha {
            Arc::new(Mutex::new("fake".into()))
        }
        fn build(&self, q: &Question) -> Result<Box<dyn EngineRow>, String> {
            let (options, state) = match q {
                Question::Choice {
                    candidates, state, ..
                } => (
                    candidates
                        .iter()
                        .map(|c| format!("{}={}", c.id, c.text))
                        .collect(),
                    state,
                ),
                Question::Bool { state, .. } => (vec!["no".to_owned(), "yes".to_owned()], state),
                Question::Score { levels, state, .. } => (levels.clone(), state),
            };
            if state.contains("BUILDFAIL") {
                return Err("fake: cannot render".into());
            }
            Ok(Box::new(FakeRow {
                ids: vec![1; state.len() / 4 + options.len()],
                options,
                state: state.clone(),
                cut: state.contains("CUT"),
                lost: state.contains("LOST"),
            }))
        }
        fn run(&mut self, rows: &[Box<dyn EngineRow>]) -> anyhow::Result<Vec<Scores>> {
            std::thread::sleep(self.delay);
            let rows: Vec<&FakeRow> = rows
                .iter()
                .map(|r| r.as_any().downcast_ref::<FakeRow>().expect("fake row"))
                .collect();
            let mut seen = self.seen.lock();
            seen.batches
                .push(rows.iter().map(|r| r.state.clone()).collect());
            seen.options.extend(rows.iter().map(|r| r.options.clone()));
            Ok(rows
                .iter()
                .map(|r| {
                    let k = r.options.len();
                    let hit = r.options.iter().position(|o| {
                        let text = o.split_once('=').map_or(o.as_str(), |(_, t)| t);
                        r.state.contains(text)
                    });
                    let probs: Vec<f32> = match hit {
                        Some(i) => (0..k)
                            .map(|j| if j == i { 0.8 } else { 0.2 / (k - 1) as f32 })
                            .collect(),
                        None => vec![1.0 / k as f32; k],
                    };
                    Scores {
                        logits: probs.iter().map(|p| p.ln()).collect(),
                        probs,
                        temperature: 1.0,
                        act_p: 0.0,
                    }
                })
                .collect())
        }
        fn load_time(&self) -> Duration {
            Duration::from_millis(1)
        }
    }

    /// A `tool` family (and, when `browser`, a `browser.target` one) on checkpoint `name`,
    /// a registered engine over the fake oracle, and the tool over the same config.
    struct Rig {
        decide: Arc<Decide>,
        engine: kameo::actor::ActorRef<Engine>,
        seen: Arc<Mutex<Seen>>,
        log: Log,
        _dir: tempfile::TempDir,
    }

    static RIGS: AtomicUsize = AtomicUsize::new(0);

    async fn rig(delay: Duration, browser: bool, patch: impl FnOnce(&mut DecisionConfig)) -> Rig {
        let name = format!(
            "t-{}-{}",
            std::process::id(),
            RIGS.fetch_add(1, Ordering::SeqCst)
        );
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("models").join(&name);
        for f in model_files(Kind::Laya) {
            let p = model.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let mut cfg = DecisionConfig::default();
        cfg.checkpoints
            .insert(name.clone(), Checkpoint::from(model.clone()));
        let mut tool = crate::FamilyGate::tool();
        tool.checkpoint = Some(name.clone());
        cfg.families.insert(TOOL_FAMILY.to_owned(), tool);
        if browser {
            let mut b = crate::FamilyGate::browser(0.25);
            b.checkpoint = Some(name.clone());
            cfg.families.insert("browser.target".to_owned(), b);
        }
        patch(&mut cfg);
        let seen = Arc::new(Mutex::new(Seen::default()));
        let loader: Loader = {
            let seen = Arc::clone(&seen);
            Arc::new(move |_, _, _| {
                Ok(Box::new(FakeOracle {
                    delay,
                    seen: Arc::clone(&seen),
                }) as Box<dyn Oracle>)
            })
        };
        let log = Log::new(dir.path().join("decisions"));
        let engine = Engine::spawn(
            Engine::with_loader(
                cfg.clone(),
                dir.path().join("models/decision"),
                log.clone(),
                loader,
            )
            .with_checkpoint(&name, model),
        );
        engine.register(actor_name(&name)).unwrap();
        Rig {
            decide: Arc::new(Decide::new(cfg, &dir.path().join("decisions"))),
            engine,
            seen,
            log,
            _dir: dir,
        }
    }

    fn choice(labels: &[&str], ids: Option<&[&str]>, state: &str) -> Q {
        Q::Choice {
            instructions: "which folder".into(),
            labels: labels.iter().map(|s| (*s).to_owned()).collect(),
            ids: ids.map(|ids| ids.iter().map(|s| (*s).to_owned()).collect()),
            state: state.into(),
        }
    }

    fn cx(dir: &std::path::Path) -> ToolCx {
        ToolCx {
            session: runtime::SessionId::new(),
            call_id: "c1".into(),
            quiet: false,
            resources: Arc::new(tools::SessionResources::durable(dir)),
        }
    }

    #[tokio::test]
    async fn shapes_ids_errors_and_order_through_the_tool() {
        let r = rig(Duration::ZERO, false, |c| {
            c.tool.batch = 2;
            c.batch_forward = true; // one forward per `AskMany`, so the fake sees the batches
        })
        .await;
        let questions = vec![
            // 0: ids are the keys; the model reads the labels.
            choice(
                &["invoices and receipts", "recruiting"],
                Some(&["inv", "rec"]),
                "your invoices and receipts for March",
            ),
            // 1: no ids → the labels are the keys; nothing matches → uniform, `none` included.
            choice(&["sports", "weather"], None, "quarterly numbers"),
            // 2: bool
            Q::Bool {
                statement: "it is spam".into(),
                state: "yes yes yes".into(),
            },
            // 3: score
            Q::Score {
                instructions: "urgency".into(),
                levels: vec!["low".into(), "high".into()],
                state: "high priority".into(),
            },
            // 4: per-row argument error
            choice(&["a", "b"], Some(&["only-one"]), "x"),
            // 5: state over the cap
            choice(&["a", "b"], None, &"s".repeat(5000)),
            // 6: a cut state still answers, flagged
            choice(&["a", "b"], None, "CUT a"),
            // 7: a lost marker is a row error
            choice(&["a", "b"], None, "LOST b"),
            // 8: a build failure is a row error
            choice(&["a", "b"], None, "BUILDFAIL"),
            // 9: reserved id
            choice(&["a", "b"], Some(&["none", "b"]), "x"),
        ];
        let out = DecideManyTool(Arc::clone(&r.decide))
            .call(
                DecideManyArgs {
                    questions,
                    checkpoint: None,
                    family: None,
                    capture: None,
                    budget_ms: None,
                },
                cx(r._dir.path()),
            )
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let rows: Vec<RowOut> = serde_json::from_value(out.structured.unwrap()).unwrap();
        assert_eq!(rows.len(), 10);
        assert!(
            out.content
                .starts_with("10 rows: 5 answered, 5 errors, 1 truncated;"),
            "{}",
            out.content
        );

        assert_eq!(rows[0].choice.as_deref(), Some("inv"));
        assert_eq!(
            rows[0].probs.keys().cloned().collect::<Vec<_>>(),
            ["inv", "none", "rec"]
        );
        assert!((rows[0].probs["inv"] - 0.8).abs() < 1e-6);
        assert!(rows[0].confidence > 0.0 && !rows[0].calibrated && !rows[0].truncated);
        assert!(!rows[0].log_id.is_empty() && rows[0].error.is_none());

        assert_eq!(
            rows[1].probs.keys().cloned().collect::<Vec<_>>(),
            ["none", "sports", "weather"]
        );
        assert!((rows[1].probs["sports"] - 1.0 / 3.0).abs() < 1e-6);

        assert_eq!(rows[2].choice.as_deref(), Some("true"));
        assert_eq!(
            rows[2].probs.keys().cloned().collect::<Vec<_>>(),
            ["false", "true"]
        );
        assert_eq!(rows[3].choice.as_deref(), Some("1"));
        assert_eq!(
            rows[3].probs.keys().cloned().collect::<Vec<_>>(),
            ["0", "1"]
        );

        assert_eq!(
            rows[4].error.as_deref(),
            Some("1 ids for 2 labels — one id per label")
        );
        assert_eq!(
            rows[5].error.as_deref(),
            Some("state is 5,000 bytes; cap 4,096 — pass a slice (sender, subject, first lines)")
        );
        assert!(rows[6].truncated && rows[6].error.is_none());
        assert_eq!(rows[6].choice.as_deref(), Some("a"));
        assert!(
            rows[7].error.as_deref().unwrap().starts_with("truncated:"),
            "{:?}",
            rows[7]
        );
        assert!(rows[7].choice.is_none() && rows[7].probs.is_empty());
        assert!(
            rows[8].error.as_deref().unwrap().contains("cannot render"),
            "{:?}",
            rows[8]
        );
        assert_eq!(
            rows[9].error.as_deref(),
            Some("id `none` is reserved for the none control")
        );

        // The model never saw the caller's keys: positional ids plus `none`, label texts.
        let seen = r.seen.lock();
        for opts in &seen.options {
            assert!(
                opts.iter()
                    .all(|o| !o.starts_with("inv=") && !o.starts_with("rec=")),
                "{opts:?}"
            );
        }
        assert!(
            seen.options
                .iter()
                .any(|o| o.contains(&"0=invoices and receipts".to_owned()))
        );
        // Batches of ≤ 2, shortest rows first (rendered length): 7 askable rows → 4 `AskMany`
        // → 4 forwards (the build failure drops out of its batch, its partner still runs).
        assert_eq!(seen.batches.len(), 4, "{:?}", seen.batches);
        assert!(seen.batches.iter().all(|b| b.len() <= 2));
        let lens: Vec<usize> = seen.batches.iter().flatten().map(String::len).collect();
        let mut sorted = lens.clone();
        sorted.sort_unstable();
        assert_eq!(lens, sorted, "batches run shortest first");
        drop(seen);
        // Logged under `tool`; the checkpoint was released at the end (nothing else holds it).
        assert_eq!(r.log.stats(&TOOL_FAMILY.into()).unwrap().asks, 7);
        assert!(r.engine.ask(LoadedNow).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn caps_and_bad_families_fail_the_call() {
        let r = rig(Duration::ZERO, false, |c| c.tool.max_rows = 3).await;
        let q = || choice(&["a", "b"], None, "a");
        let err = r
            .decide
            .run(vec![q(), q(), q(), q()], None, None, None, None)
            .await
            .unwrap_err();
        assert_eq!(err, "decide_many: 4 rows > max_rows 3 — loop over slices");
        let err = r
            .decide
            .run(vec![q()], None, Some("meeting.kind"), None, None)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "family must be `tool` or `tool.<task>`, not `meeting.kind`"
        );
        let err = r
            .decide
            .run(vec![q()], Some("nowhere"), None, None, None)
            .await
            .unwrap_err();
        assert!(
            err.starts_with("checkpoint `nowhere` has no engine"),
            "{err}"
        );
        let err = r
            .decide
            .run(vec![q()], None, None, Some("../x"), None)
            .await
            .unwrap_err();
        assert!(err.starts_with("capture must name a task"), "{err}");
        // The Tool surface reports those as error output, not `Err`.
        let out = DecideTool(Arc::clone(&r.decide))
            .call(
                DecideArgs {
                    question: q(),
                    checkpoint: Some("nowhere".into()),
                    family: None,
                    capture: None,
                    budget_ms: None,
                },
                cx(r._dir.path()),
            )
            .await
            .unwrap();
        assert!(out.is_error && out.structured.is_none());
    }

    /// 92 §2: a configured `tool.<task>` is asked under its own name; an unknown one answers
    /// as `tool` with a note; `decide` returns the one row as a dict.
    #[tokio::test]
    async fn sub_family_resolution() {
        let r = rig(Duration::ZERO, false, |c| {
            let mut mail = c.families[TOOL_FAMILY].clone();
            mail.temperature = 2.0;
            c.families.insert("tool.mail".to_owned(), mail);
        })
        .await;
        let (rows, summary) = r
            .decide
            .run(
                vec![choice(&["a", "b"], None, "a")],
                None,
                Some("tool.mail"),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(rows[0].calibrated, "temperature 2.0 → calibrated: {rows:?}");
        assert!(rows[0].probs["a"] < 0.8 && rows[0].choice.as_deref() == Some("a"));
        assert!(summary.contains("family tool.mail,"), "{summary}");
        assert_eq!(r.log.stats(&"tool.mail".into()).unwrap().asks, 1);
        let out = DecideTool(Arc::clone(&r.decide))
            .call(
                DecideArgs {
                    question: choice(&["a", "b"], None, "b"),
                    checkpoint: None,
                    family: Some("tool.nope".into()),
                    capture: None,
                    budget_ms: None,
                },
                cx(r._dir.path()),
            )
            .await
            .unwrap();
        assert!(
            out.content
                .ends_with("family `tool.nope` is not configured; answered as `tool`"),
            "{}",
            out.content
        );
        let row: RowOut = serde_json::from_value(out.structured.unwrap()).unwrap();
        assert_eq!(row.choice.as_deref(), Some("b"));
        assert!(!row.calibrated);
        assert_eq!(r.log.stats(&TOOL_FAMILY.into()).unwrap().asks, 1);
    }

    /// 92 §6: `capture` writes one example per answered row keyed by its `log_id`;
    /// `decide_label` appends the label; a row from a call without capture cannot be labelled.
    #[tokio::test]
    async fn capture_and_label_write_training_rows() {
        let r = rig(Duration::ZERO, false, |_| {}).await;
        let (rows, summary) = r
            .decide
            .run(
                vec![
                    choice(
                        &["invoices", "recruiting"],
                        Some(&["inv", "rec"]),
                        "an invoices mail",
                    ),
                    choice(&["a"], Some(&["x", "y"]), "bad row"),
                ],
                None,
                None,
                Some("mail"),
                None,
            )
            .await
            .unwrap();
        assert!(summary.ends_with("captured under tool.mail"), "{summary}");
        let store = training::Store::new(r._dir.path().join("decisions/training"));
        let read = store.read("tool.mail", None).unwrap();
        assert_eq!(read.len(), 1, "only the answered row: {read:?}");
        let training::Row::Example(e) = &read[0] else {
            panic!("{read:?}")
        };
        assert_eq!(e.id.to_string(), rows[0].log_id);
        assert_eq!(
            (e.family.as_str(), e.text.as_str()),
            ("tool.mail", "an invoices mail")
        );
        assert_eq!(e.questions.len(), 1);
        let asked = &e.questions[0];
        assert_eq!(asked.kind, "choice");
        assert_eq!(
            asked
                .candidates
                .iter()
                .map(|c| (c.id.as_str(), c.text.as_str()))
                .collect::<Vec<_>>(),
            [("inv", "invoices"), ("rec", "recruiting")]
        );
        assert_eq!(
            asked.probabilities.keys().cloned().collect::<Vec<_>>(),
            ["inv", "none", "rec"]
        );
        assert_eq!(asked.family, "tool");

        let out = DecideLabelTool(Arc::clone(&r.decide))
            .call(
                DecideLabelArgs {
                    log_id: rows[0].log_id.clone(),
                    id: "rec".into(),
                },
                cx(r._dir.path()),
            )
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.structured.unwrap(),
            serde_json::json!({ "ok": true, "family": "tool.mail" })
        );
        let folded = training::fold(store.read("tool.mail", None).unwrap());
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].label.as_deref(), Some("rec"));

        // Not captured → the exact error; a non-uuid → a parse error.
        let (rows, _) = r
            .decide
            .run(vec![choice(&["a", "b"], None, "a")], None, None, None, None)
            .await
            .unwrap();
        assert_eq!(
            r.decide.label(&rows[0].log_id, "a").unwrap_err(),
            "row was not captured; pass capture=\"tool.<task>\" to decide_many"
        );
        assert!(
            r.decide
                .label("nope", "a")
                .unwrap_err()
                .starts_with("log_id:")
        );
    }

    /// 92 §3: a browser-family ask queued while a sort is running is answered before the sort's
    /// last batch (one `AskMany` per batch, the mailbox interleaves); and the checkpoint stays
    /// resident afterwards because the browser family holds it.
    #[tokio::test]
    async fn other_families_interleave_and_hold_the_checkpoint() {
        let r = rig(Duration::from_millis(40), true, |c| c.tool.batch = 1).await;
        let questions: Vec<Q> = (0..6)
            .map(|i| choice(&["a", "b"], None, &format!("row {i} a")))
            .collect();
        let decide = Arc::clone(&r.decide);
        let sort = tokio::spawn(async move { decide.run(questions, None, None, None, None).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let d = r
            .engine
            .ask(Ask::new(
                "browser.target",
                Question::Choice {
                    instructions: "which".into(),
                    candidates: vec![Candidate {
                        id: "e1".into(),
                        text: "BROWSER".into(),
                    }],
                    state: "BROWSER please".into(),
                },
            ))
            .await
            .unwrap();
        assert_eq!(d.choice, "e1");
        let (rows, _) = sort.await.unwrap().unwrap();
        assert_eq!(rows.len(), 6);
        assert!(
            rows.iter()
                .all(|r| r.choice.as_deref() == Some("a") && r.error.is_none())
        );
        let seen = r.seen.lock();
        let browser_at = seen
            .batches
            .iter()
            .position(|b| b[0].starts_with("BROWSER"))
            .expect("the browser ask ran");
        assert!(
            browser_at > 0 && browser_at < seen.batches.len() - 1,
            "browser ask at forward {browser_at} of {}",
            seen.batches.len()
        );
        drop(seen);
        assert_eq!(
            r.engine.ask(LoadedNow).await.unwrap().len(),
            1,
            "browser.target holds it"
        );
    }

    /// `budget_ms` over → every row of the batch reports the budget error, nothing panics.
    #[tokio::test]
    async fn budget_errors_per_row() {
        let r = rig(Duration::from_millis(200), false, |c| c.tool.batch = 4).await;
        let (rows, _) = r
            .decide
            .run(
                vec![choice(&["a", "b"], None, "a"); 2],
                None,
                None,
                None,
                Some(20),
            )
            .await
            .unwrap();
        for row in &rows {
            assert_eq!(
                row.error.as_deref(),
                Some(
                    "budget: the batch did not finish in 20 ms — raise budget_ms or pass fewer rows"
                ),
                "{row:?}"
            );
        }
    }
}
