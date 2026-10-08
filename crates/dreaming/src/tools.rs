//! The nine `dreaming` tools (design doc 136 §9.4, contract §2.4). Wave C
//! registers every one with its name, Risk, args and RETURNS; bodies answer
//! `not built: W3` except `dreaming_state`, which reads `state.json`. Every
//! mutating tool runs only in the dreaming routine's session or the owner's
//! own root session (the knowledge `Consolidator` gate, 136 §2.1).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::bail;
use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::config::DreamConfig;
use crate::types::{Case, CaseState, EditRecord, NightId, Split};

/// What every tool needs: the data dir, the config, and the gate.
pub struct Seams {
    /// `<data_dir>`.
    pub data_dir: PathBuf,
    /// The section as read at start.
    pub cfg: DreamConfig,
    /// The knowledge store, for the owner-root-or-unattended gate.
    pub knowledge: Option<Arc<knowledge::Knowledge>>,
}

impl Seams {
    /// 136 §2.1: the dreaming routine's session or the owner's own root.
    async fn mutating(&self, name: &str, cx: &ToolCx) -> anyhow::Result<()> {
        anyhow::ensure!(
            !cx.resources.is_private(),
            "{name} is disabled in a private session"
        );
        let ok = match &self.knowledge {
            Some(k) => k.may_consolidate(cx.session).await,
            None => false,
        };
        anyhow::ensure!(
            ok,
            "{name} runs only in the dreaming routine's session or the owner's own root session (136 §2.1)"
        );
        Ok(())
    }
}

/// Register the tools and the prompt block.
pub async fn register(reg: &ActorRef<ToolRegistry>, seams: Arc<Seams>) {
    if let Some(block) = crate::preferences::block(&seams.data_dir) {
        let _ = reg
            .ask(RegisterPrompt {
                plugin: "dreaming",
                text: block,
            })
            .await;
    }
    let s = &seams;
    let _ = reg.ask(RegisterTool(Arc::new(LedgerTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(StateTool(s.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(CasePutTool(s.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(CaseListTool(s.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SkillLearnedEditTool(s.clone()))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(ReplayTool(s.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(PreferencesSetTool(s.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(JournalTool(s.clone()))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(UndoTool(s.clone())))).await;
}

/// Text for the transcript, the same data as a Python value for the runtime.
fn structured<T: Serialize>(text: String, data: T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

/// Arguments for [`LedgerTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct LedgerArgs {
    /// RFC 3339 window start; default `state.last_dream_at` or 24 h ago.
    #[serde(default)]
    pub since: Option<String>,
}

/// `dreaming_ledger`: the day ledger (136 §4 phase 1).
pub struct LedgerTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for LedgerTool {
    const NAME: &'static str = "dreaming_ledger";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = LedgerArgs;

    fn description(&self) -> String {
        "Dreaming: the day ledger since the last night — sessions, plain user messages, tool errors, questions, \
         stops, app presses, hint impressions/opens, skill loads, rewinds, pending desk journals and any \
         `run_request`. Read it first; it decides tonight's phases."
            .into()
    }

    async fn call(&self, a: LedgerArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let l = crate::ledger::build(&self.0.data_dir, a.since.as_deref()).await?;
        Ok(structured(
            format!(
                "ledger {}..{}: {} sessions",
                l.ledger.since,
                l.ledger.until,
                l.ledger.sessions.len()
            ),
            l,
        ))
    }
}

/// No arguments.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NoArgs {}

/// `dreaming_state`: `DreamStatus` from `state.json` (real in wave C).
pub struct StateTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for StateTool {
    const NAME: &'static str = "dreaming_state";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = NoArgs;

    fn description(&self) -> String {
        "Dreaming: status — enabled, the last night's counts and spend, the next window, tonight's budget left, \
         the backlog, proposals pending, paused_until."
            .into()
    }

    async fn call(&self, _: NoArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let st = crate::night::status(&self.0.data_dir, &self.0.cfg);
        let text = match &st.last_night {
            Some(n) => format!(
                "last night {}: {} merged, {} archived, {} edits kept, ${:.2}; budget left tonight ${:.2}",
                n.night,
                n.counts.merged,
                n.counts.archived,
                n.counts.edits_accepted,
                n.spend_usd,
                st.budget_left_tonight_usd
            ),
            None => format!(
                "no night yet; budget tonight ${:.2}",
                st.budget_left_tonight_usd
            ),
        };
        Ok(structured(text, st))
    }
}

/// Arguments for [`CasePutTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CasePutArgs {
    /// The case (136 §6 shape); `id`, `night`, `split`, `state` are set by the harness when empty.
    pub case: Case,
}

/// `case_put`: append one case to `cases.jsonl`.
pub struct CasePutTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for CasePutTool {
    const NAME: &'static str = "case_put";
    const RISK: Risk = Risk::Mutating;
    type Args = CasePutArgs;

    fn description(&self) -> String {
        "Dreaming only: record one case — a unit of hindsight (intent, context excerpt, what happened, the \
         user's words, the ideal, tags, skill_hint). Returns the case id. Secrets are scrubbed; raw transcripts \
         are never stored."
            .into()
    }

    async fn call(&self, a: CasePutArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.mutating(Self::NAME, &cx).await?;
        crate::cases::append(&self.0.data_dir, &a.case)?;
        Ok(ToolOutput::ok(a.case.id))
    }
}

/// Arguments for [`CaseListTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CaseListArgs {
    /// `open | closed | reopened | settled`.
    #[serde(default)]
    pub state: Option<CaseState>,
    /// Exact `skill_hint`.
    #[serde(default)]
    pub skill_hint: Option<String>,
    /// `train | val`.
    #[serde(default)]
    pub split: Option<Split>,
    /// Max rows (default 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `case_list`: filtered cases.
pub struct CaseListTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for CaseListTool {
    const NAME: &'static str = "case_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = CaseListArgs;

    fn description(&self) -> String {
        "Dreaming: list cases by state, skill_hint and split (newest first).".into()
    }

    async fn call(&self, a: CaseListArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cases = crate::cases::list(
            &self.0.data_dir,
            a.state,
            a.skill_hint.as_deref(),
            a.split,
            a.limit.unwrap_or(100) as usize,
        )?;
        Ok(structured(format!("{} cases", cases.len()), cases))
    }
}

/// Arguments for [`SkillLearnedEditTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SkillLearnedEditArgs {
    /// Skill name; only `skills/learned/<skill>.md` is written.
    pub skill: String,
    /// ≤ 4 bounded edits.
    pub edits: Vec<EditRecord>,
    /// The night id (the backup directory).
    pub night: NightId,
}

/// `skill_learned_edit`: apply bounded edits to a learned sidecar.
pub struct SkillLearnedEditTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for SkillLearnedEditTool {
    const NAME: &'static str = "skill_learned_edit";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = SkillLearnedEditArgs;

    fn description(&self) -> String {
        "Dreaming only: apply ≤ 4 bounded edits (add | replace | delete with exact-text anchors) to a skill's \
         learned sidecar `skills/learned/<skill>.md`; the file is backed up first. Returns the apply report \
         (applied | skipped_unmatched | error per edit) and the candidate text. Never touches the skill itself."
            .into()
    }

    async fn call(&self, a: SkillLearnedEditArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.mutating(Self::NAME, &cx).await?;
        if a.edits.len() > self.0.cfg.caps.edits_per_target as usize {
            bail!(
                "{} edits; the cap is {} per target",
                a.edits.len(),
                self.0.cfg.caps.edits_per_target
            );
        }
        let r = crate::edits::apply(&self.0.data_dir, &a.skill, &a.edits, &a.night)?;
        Ok(structured(
            format!("{}: {} edits", r.target, r.edits.len()),
            r,
        ))
    }
}

/// Arguments for [`ReplayTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReplayArgs {
    /// The case to replay.
    pub case_id: String,
    /// The candidate sidecar text; `None` = the current text (the baseline run).
    #[serde(default)]
    pub candidate_text: Option<String>,
    /// `provider/model` for the answer; `None` = the session's model. The judge is fixed by config.
    #[serde(default)]
    pub target_model: Option<String>,
}

/// `replay_case`: answer a case in a fresh context and judge it.
pub struct ReplayTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for ReplayTool {
    const NAME: &'static str = "replay_case";
    // ReadOnly: writes nothing; spends model budget under the session's cap.
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = ReplayArgs;

    fn description(&self) -> String {
        "Dreaming: replay one case — render its context with the candidate skill text, answer it with the target \
         model in a fresh context, judge the answer against the ideal and the user's words with the configured \
         judge. Returns {answer, score (0–1), reason}. Spends model budget."
            .into()
    }

    async fn call(&self, a: ReplayArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = crate::replay::replay(
            &self.0.data_dir,
            &a.case_id,
            a.candidate_text.as_deref(),
            a.target_model.as_deref(),
        )
        .await?;
        Ok(structured(format!("score {:.2}: {}", r.score, r.reason), r))
    }
}

/// Arguments for [`PreferencesSetTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PreferencesSetArgs {
    /// `add | replace | demote`.
    pub op: String,
    /// The line (≤ 140 chars).
    pub line: String,
    /// Session ids / `user` backing it.
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// `preferences_set`: edit the standing preferences block.
pub struct PreferencesSetTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for PreferencesSetTool {
    const NAME: &'static str = "preferences_set";
    const RISK: Risk = Risk::Mutating;
    type Args = PreferencesSetArgs;

    fn description(&self) -> String {
        "Dreaming only: add, replace or demote one line of the standing preferences block (≤ 20 lines × 140 \
         chars, in the system prompt every turn). Refuses past the cap. Returns the block text."
            .into()
    }

    async fn call(&self, a: PreferencesSetArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.mutating(Self::NAME, &cx).await?;
        let block = crate::preferences::set(
            &self.0.data_dir,
            &a.op,
            &a.line,
            &a.evidence,
            self.0.cfg.preferences.cap_lines,
        )
        .await?;
        Ok(ToolOutput::ok(block))
    }
}

/// Arguments for [`JournalTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct JournalArgs {
    /// The night id.
    pub night: NightId,
    /// Section heading inside tonight's entry.
    pub section: String,
    /// Markdown.
    pub markdown: String,
}

/// `dreaming_journal`: append to the `Dreaming` note.
pub struct JournalTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for JournalTool {
    const NAME: &'static str = "dreaming_journal";
    const RISK: Risk = Risk::Mutating;
    type Args = JournalArgs;

    fn description(&self) -> String {
        "Dreaming only: append markdown under a section of tonight's entry in the `Dreaming` note (counts, \
         changed groups with their undo handles, proposals, spend, the meta-note diff)."
            .into()
    }

    async fn call(&self, a: JournalArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.mutating(Self::NAME, &cx).await?;
        crate::journal::append(&self.0.data_dir, &a.night, &a.section, &a.markdown).await?;
        Ok(ToolOutput::ok(format!(
            "journal {}: {}",
            a.night, a.section
        )))
    }
}

/// Arguments for [`UndoTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UndoArgs {
    /// The night id.
    pub night: NightId,
    /// A group of the night; absent = the whole night.
    #[serde(default)]
    pub group: Option<String>,
}

/// `dreaming_undo`: revert a night or a group of it.
pub struct UndoTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for UndoTool {
    const NAME: &'static str = "dreaming_undo";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = UndoArgs;

    fn description(&self) -> String {
        "Undo a dreaming night (or one group of it): restore every knowledge concept it changed to its pre-night \
         revision, restore the learned sidecars from backup, mark the edits undone. Returns the UndoReport."
            .into()
    }

    async fn call(&self, a: UndoArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.mutating(Self::NAME, &cx).await?;
        let r = crate::undo::undo(&self.0.data_dir, &a.night, a.group.as_deref()).await?;
        Ok(structured(
            format!(
                "undone {}: {} concepts, {} skills",
                a.night,
                r.knowledge_restored.len(),
                r.skills_restored.len()
            ),
            r,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_and_risks_match_the_contract() {
        assert_eq!(
            [
                LedgerTool::NAME,
                StateTool::NAME,
                CasePutTool::NAME,
                CaseListTool::NAME,
                SkillLearnedEditTool::NAME,
                ReplayTool::NAME,
                PreferencesSetTool::NAME,
                JournalTool::NAME,
                UndoTool::NAME,
            ],
            [
                "dreaming_ledger",
                "dreaming_state",
                "case_put",
                "case_list",
                "skill_learned_edit",
                "replay_case",
                "preferences_set",
                "dreaming_journal",
                "dreaming_undo",
            ]
        );
        for r in [
            LedgerTool::RISK,
            StateTool::RISK,
            CaseListTool::RISK,
            ReplayTool::RISK,
        ] {
            assert_eq!(r, Risk::ReadOnly);
        }
        for r in [
            CasePutTool::RISK,
            SkillLearnedEditTool::RISK,
            PreferencesSetTool::RISK,
            JournalTool::RISK,
            UndoTool::RISK,
        ] {
            assert_eq!(r, Risk::Mutating);
        }
    }

    fn cx(private: bool) -> ToolCx {
        ToolCx {
            session: runtime::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: Arc::new(tools::SessionResources {
                root: std::env::temp_dir(),
                retention: if private {
                    tools::Retention::Private
                } else {
                    tools::Retention::Durable
                },
                generation: 1,
            }),
        }
    }

    /// `dreaming_state` is real; the mutating tools refuse without the gate.
    #[tokio::test]
    async fn state_is_real_and_mutating_tools_need_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let seams = Arc::new(Seams {
            data_dir: dir.path().to_owned(),
            cfg: DreamConfig::default(),
            knowledge: None,
        });
        let out = StateTool(seams.clone())
            .call(NoArgs {}, cx(false))
            .await
            .unwrap();
        assert!(out.content.starts_with("no night yet"), "{}", out.content);
        assert_eq!(out.structured.unwrap()["budget_usd"], 1.0);
        let e = UndoTool(seams.clone())
            .call(
                UndoArgs {
                    night: "2026-10-08".into(),
                    group: None,
                },
                cx(false),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("dreaming_undo runs only"), "{e}");
        let e = JournalTool(seams.clone())
            .call(
                JournalArgs {
                    night: "n".into(),
                    section: "s".into(),
                    markdown: "m".into(),
                },
                cx(true),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("private session"), "{e}");
        let e = ReplayTool(seams)
            .call(
                ReplayArgs {
                    case_id: "x".into(),
                    candidate_text: None,
                    target_model: None,
                },
                cx(false),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("not built: W3"), "{e}");
    }
}
