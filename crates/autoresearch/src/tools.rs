//! W6 — the seven tools (design doc 140 §12.4). The four study tools
//! (`experiment_begin|run|log|notes`) are visible only through the study
//! `Toolset` and refused elsewhere; the three root tools
//! (`autoresearch_draft|status|control`) serve the user's chats. Every body
//! asks the [`Studies`] actor; `autoresearch_draft` renders the draft card
//! through `crates/app` (W3's `study_draft.rs`). Registration and the
//! descriptions (verbatim from §12.4) are the contract; bodies are W6's.

use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::config::{AgentDefaults, AutoresearchConfig};
use crate::error::Error;
use crate::types::{
    Begin, Control, ControlOp, Log, LogArgs, Notes, NotesArgs, Run, RunArgs, Status, StudySpec,
};
use crate::{AUTORESEARCH_ACTOR, PLUGIN, Studies};

/// What every tool needs.
pub struct Seams {
    /// `<data_dir>`.
    pub data_dir: std::path::PathBuf,
    /// The section as read at start.
    pub cfg: AutoresearchConfig,
    /// The `agent:` section's model defaults (R1-D5).
    pub agent: AgentDefaults,
}

/// The config-only prompt block (140 §8.3, 07 §: a function of config only).
pub const PROMPT_BLOCK: &str = "An autoresearch study (design 140) is an unattended, measured experiment loop: the user names a goal, \
    a benchmark that prints a number, a correctness check and the files that may change; theuth runs one idea per \
    experiment in its own git worktree, measures it against the noise, keeps only what clears the gate and reverts the \
    rest with the lesson written down. The study tools experiment_begin, experiment_run, experiment_log and \
    experiment_notes exist only inside a study session (a fire of the schedule autoresearch-<slug>); their brief carries \
    the whole protocol. In a normal session use autoresearch_draft to propose a study for the user to approve, \
    autoresearch_status to see studies and their results, and autoresearch_control to pause, resume, stop, steer or \
    add ideas; the /autoresearch skill explains how to set one up.";

/// Register the seven tools and the prompt block.
pub async fn register(reg: &ActorRef<ToolRegistry>, seams: Arc<Seams>) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: PLUGIN,
            text: PROMPT_BLOCK.to_owned(),
        })
        .await;
    let s = &seams;
    let _ = reg.ask(RegisterTool(Arc::new(BeginTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(RunTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(LogTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(NotesTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(DraftTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(StatusTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ControlTool(s.clone())))).await;
}

/// Text for the transcript, the same data as a Python value for the runtime.
pub fn structured<T: Serialize>(text: String, data: T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

/// The actor, or the "not running" refusal.
pub fn actor() -> anyhow::Result<ActorRef<Studies>> {
    ActorRef::<Studies>::lookup(AUTORESEARCH_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("autoresearch is not running"))
}

/// No arguments.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NoArgs {}

/// `experiment_begin` (140 §12.4).
pub struct BeginTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for BeginTool {
    const NAME: &'static str = "experiment_begin";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = NoArgs;

    fn description(&self) -> String {
        "Start the next experiment of this autoresearch study. Returns the brief: goal, incumbent, noise, recent \
         experiments, dead ends, ideas, the user's notes, budget left, and the protocol. Call it first."
            .into()
    }

    async fn call(&self, _: NoArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let brief = actor()?
            .ask(Begin {
                session: cx.session,
                slot: None,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(structured(brief.text.clone(), brief))
    }
}

/// `experiment_run` (140 §12.4).
pub struct RunTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for RunTool {
    const NAME: &'static str = "experiment_run";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = RunArgs;

    fn description(&self) -> String {
        "Measure your current change in this autoresearch study. The harness checks the evaluator files and scope, \
         runs the study's fixed benchmark (1–3 samples, minutes), runs the correctness check if the change would be \
         kept, and returns a verdict computed from the measured noise. Call it once per idea, after your edit; after a \
         crash you may fix and call it again (the brief says how many times). Do not run the benchmark yourself for \
         scoring."
            .into()
    }

    async fn call(&self, args: RunArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let report = actor()?
            .ask(Run {
                session: cx.session,
                slot: None,
                args,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(structured(report.line.clone(), report))
    }
}

/// `experiment_log` (140 §12.4).
pub struct LogTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for LogTool {
    const NAME: &'static str = "experiment_log";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = LogArgs;

    fn description(&self) -> String {
        "Close the current experiment. The harness applies the verdict of the last experiment_run: an improvement is \
         committed, anything else is reverted (your change is saved as a patch). Write what you did and what the \
         result teaches the next experiment. decision 'discard' throws away an improvement you distrust; \
         'keep_simpler' keeps a change that simplifies the code at equal performance (the harness verifies it)."
            .into()
    }

    async fn call(&self, args: LogArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let report = actor()?
            .ask(Log {
                session: cx.session,
                slot: None,
                args,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = format!(
            "{:?}; incumbent {} ({} keeps)",
            report.applied, report.incumbent.sha, report.incumbent.kept
        );
        Ok(structured(text, report))
    }
}

/// `experiment_notes` (140 §12.4).
pub struct NotesTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for NotesTool {
    const NAME: &'static str = "experiment_notes";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = NotesArgs;

    fn description(&self) -> String {
        "Update the study's playbook (replaces it; keep it under 4 KiB) or add ideas to the backlog for later \
         experiments."
            .into()
    }

    async fn call(&self, args: NotesArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let report = actor()?
            .ask(Notes {
                session: cx.session,
                args,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = format!(
            "playbook {} bytes; {} ideas added",
            report.playbook_bytes, report.ideas_added
        );
        Ok(structured(text, report))
    }
}

/// `autoresearch_draft` (140 §12.4): the spec → the draft card. Read-only:
/// nothing is written until the press (the Start buttons call
/// `autoresearch.create {draft_id, start}`).
pub struct DraftTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for DraftTool {
    const NAME: &'static str = "autoresearch_draft";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = StudySpec;

    fn description(&self) -> String {
        "Propose an autoresearch study for the user to approve: the goal, the metric and its direction, the \
         benchmark command, the correctness check, an optional holdout, the scope and off-limits paths, budgets. \
         Shows a draft card; the user's Start press creates the study — this call writes nothing. Benchmark scripts \
         written during setup belong under the autoresearch drafts directory, never in the user's checkout."
            .into()
    }

    async fn call(&self, _spec: StudySpec, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        // W3: render the card through crates/app `study_draft` (routine_draft pattern).
        Err(Error::NotBuilt("W3").into())
    }
}

/// `autoresearch_status` arguments.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct StatusArgs {
    /// One study; omit for every study.
    #[serde(default)]
    pub slug: Option<String>,
}

/// `autoresearch_status` (140 §12.4).
pub struct StatusTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for StatusTool {
    const NAME: &'static str = "autoresearch_status";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = StatusArgs;

    fn description(&self) -> String {
        "Autoresearch studies: state, headline (metric now vs base), experiments run and kept, budget left, the \
         current phase. Name a slug for one study with its last experiments."
            .into()
    }

    async fn call(&self, a: StatusArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let studies = actor()?
            .ask(Status { slug: a.slug })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = if studies.is_empty() {
            "no autoresearch studies".to_owned()
        } else {
            studies
                .iter()
                .map(|s| format!("{}: {}", s.slug, s.headline))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(structured(text, studies))
    }
}

/// `autoresearch_control` arguments (140 §12.4): `op` includes `steer` and `idea`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ControlArgs {
    /// The study.
    pub slug: String,
    /// `pause | resume | stop_after | stop_now | run_once | segment | finalize | restore | discard | steer | idea`.
    pub op: String,
    /// For `segment` (why) and `restore`/`idea` details.
    #[serde(default)]
    pub reason: Option<String>,
    /// For `steer` (the text) and `idea` (the idea).
    #[serde(default)]
    pub text: Option<String>,
    /// For `restore`: the experiment number.
    #[serde(default)]
    pub n: Option<u32>,
}

/// `autoresearch_control` (140 §12.4).
pub struct ControlTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for ControlTool {
    const NAME: &'static str = "autoresearch_control";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = ControlArgs;

    fn description(&self) -> String {
        "Control an autoresearch study from this chat: pause, resume, stop_after (finalize after the experiment in \
         flight), stop_now (discard it), run_once, segment (new evaluator custody and baseline; give a reason), \
         finalize, restore (re-measure a reverted keep; give n), discard, steer (text reaches the next experiment \
         without pausing) or idea (add to the backlog; text, optional category in reason)."
            .into()
    }

    async fn call(&self, a: ControlArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let op = match a.op.as_str() {
            "pause" => ControlOp::Pause,
            "resume" => ControlOp::Resume,
            "stop_after" => ControlOp::StopAfter,
            "stop_now" => ControlOp::StopNow,
            "run_once" => ControlOp::RunOnce,
            "segment" => ControlOp::Segment {
                reason: a.reason.clone().unwrap_or_default(),
            },
            "finalize" => ControlOp::Finalize,
            "restore" => ControlOp::Restore {
                n: a.n.ok_or_else(|| anyhow::anyhow!("restore needs n"))?,
            },
            "discard" => ControlOp::Discard,
            "steer" | "idea" => {
                // W6: route to SteerMsg / IdeaOp.
                return Err(Error::NotBuilt("W6").into());
            }
            other => anyhow::bail!("unknown op `{other}`"),
        };
        let info = actor()?
            .ask(Control {
                slug: a.slug,
                op,
                by: crate::types::By::User,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(structured(format!("{}: {}", info.slug, info.headline), info))
    }
}
