//! The nine `dreaming` tools (design doc 136 §9.4, contract §2.4). Every
//! mutating tool runs only in the dreaming routine's session or the owner's
//! own root session (the knowledge `Consolidator` gate, 136 §2.1).
//! `dreaming_ledger` opens the night and `dreaming_journal{section: "journal"}`
//! closes it; two journal sections are protocols — `proposals` (a JSON array
//! of drafts → the inbox) and `meta-note` (replaces `state.meta_note`) — so
//! the tool table stays at nine.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::bail;
use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::config::{AgentDefaults, DreamConfig};
use crate::night::{DREAMING_ACTOR, Night, ProposalDraft};
use crate::types::{Case, CaseKind, CaseOrigin, CaseState, EditRecord, NightId, Split};

/// What every tool needs: the data dir, the config, and the gate.
pub struct Seams {
    /// `<data_dir>`.
    pub data_dir: PathBuf,
    /// The section as read at start.
    pub cfg: DreamConfig,
    /// The `agent:` section's model defaults (136 D12).
    pub agent: AgentDefaults,
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

/// `dreaming_ledger`: the day ledger (136 §4 phase 1). Mutating (R-10, D9):
/// it opens the night, consumes the pending rewinds and writes the night
/// file, so it runs behind the same gate as the other writers.
pub struct LedgerTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for LedgerTool {
    const NAME: &'static str = "dreaming_ledger";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = LedgerArgs;

    fn description(&self) -> String {
        "Dreaming only: the day ledger since the last night — sessions, plain user messages, tool errors, questions, \
         stops, app presses, hint impressions/opens, skill loads, rewinds, pending desk journals and any \
         `run_request`. Opens tonight's night. Read it first; it decides tonight's phases."
            .into()
    }

    async fn call(&self, a: LedgerArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.mutating(Self::NAME, &cx).await?;
        let l = crate::ledger::build(
            &self.0.data_dir,
            &self.0.cfg,
            self.0.knowledge.as_deref(),
            Some(cx.session),
            a.since.as_deref(),
        )
        .await?;
        Ok(structured(
            format!(
                "night {} ({}); ledger {}..{}: {} sessions, {} plain user messages, {} tool errors, {} rewinds; \
                 backlog fresh {} / untidied {} / open cases {}; tidy candidates: {} fresh, {} retitle, {} duplicate stems, \
                 {} archive-by-use, {} to verify{}",
                l.night,
                l.mode,
                l.ledger.since,
                l.ledger.until,
                l.sessions,
                l.ledger.user_messages.len(),
                l.ledger.tool_errors.len(),
                l.ledger.rewinds.len() + l.pending_rewinds.len(),
                l.backlog.fresh,
                l.backlog.untidied,
                l.backlog.open_cases,
                l.tidy.fresh.len(),
                l.tidy.retitle.len(),
                l.tidy.duplicate_stems.len(),
                l.tidy.archive_by_use.len(),
                l.tidy.verify_sample.len(),
                l.run_request.as_ref().map_or(String::new(), |r| format!(
                    "; run_request: ${} {:?}",
                    r.budget_usd, r.mode
                ))
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
        let st = match ActorRef::<Night>::lookup(DREAMING_ACTOR) {
            Ok(Some(a)) => a
                .ask(crate::night::Status)
                .await
                .unwrap_or_else(|_| crate::night::status(&self.0.data_dir, &self.0.cfg)),
            _ => crate::night::status(&self.0.data_dir, &self.0.cfg),
        };
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

/// A case as the Mine phase hands it in (136 §6 shape; the harness fills
/// `id`, `night`, `at`, `split`). An `id` of an existing case makes this a
/// state transition (`closed` with `closed_by`, `reopened`, `settled`).
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CaseDraft {
    /// Empty for a new case; an existing id to change its state.
    #[serde(default)]
    pub id: String,
    /// `correction | re-ask | disapproval | rewind | stop | denial | tool-chain | hint-miss | task-returned`.
    #[serde(default = "default_kind")]
    pub kind: CaseKind,
    /// The generalised intent.
    #[serde(default)]
    pub intent: String,
    /// ≤ 1,200 chars ending just before the mistake.
    #[serde(default)]
    pub context: String,
    /// What the assistant did.
    #[serde(default)]
    pub happened: String,
    /// The user's words, verbatim.
    #[serde(default)]
    pub user_words: String,
    /// What right would have looked like (required for a new case).
    #[serde(default)]
    pub ideal: String,
    /// Free tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// `<skill name>` | `prompt:standing-preferences` | `knowledge:<id>`.
    #[serde(default)]
    pub skill_hint: String,
    /// Where it happened.
    #[serde(default)]
    pub source_session: Option<runtime::SessionId>,
    /// RFC 3339 of the moment; empty = now.
    #[serde(default)]
    pub at: String,
    /// `real` (default) | `synthetic`.
    #[serde(default)]
    pub origin: CaseOrigin,
    /// `open` (default) | `closed` | `reopened` | `settled`.
    #[serde(default)]
    pub state: CaseState,
    /// `edit:<id>` when closing.
    #[serde(default)]
    pub closed_by: Option<String>,
}

fn default_kind() -> CaseKind {
    CaseKind::Correction
}

/// Arguments for [`CasePutTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CasePutArgs {
    /// The case.
    pub case: CaseDraft,
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
        let d = a.case;
        if !d.id.trim().is_empty()
            && let Ok(existing) = crate::cases::get(&self.0.data_dir, d.id.trim())
        {
            anyhow::ensure!(
                d.state != existing.state,
                "case {} is already {:?}",
                existing.id,
                existing.state
            );
            let c = crate::cases::transition(
                &self.0.data_dir,
                &existing.id,
                d.state,
                d.closed_by.as_deref(),
            )?;
            return Ok(ToolOutput::ok(c.id));
        }
        anyhow::ensure!(
            !d.ideal.trim().is_empty(),
            "a case needs `ideal` (what right looked like); log friction without one as a count, not a case"
        );
        anyhow::ensure!(!d.intent.trim().is_empty(), "a case needs `intent`");
        let case = Case {
            id: d.id,
            night: String::new(),
            kind: d.kind,
            intent: d.intent,
            context: d.context,
            happened: d.happened,
            user_words: d.user_words,
            ideal: d.ideal,
            tags: d.tags,
            skill_hint: d.skill_hint,
            source_session: d.source_session.unwrap_or(cx.session),
            at: d.at,
            split: Split::Train,
            origin: d.origin,
            state: d.state,
            closed_by: d.closed_by,
            reopened_at: None,
        };
        let night = crate::ledger::tonight();
        let c = crate::cases::append(&self.0.data_dir, &case, &night)?;
        Ok(ToolOutput::ok(c.id))
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
    /// Skill name (`[a-z0-9-]`); only `skills/learned/<skill>.md` is written.
    pub skill: String,
    /// ≤ 4 bounded edits; `{op, content, anchor, rationale, cases}` is enough.
    pub edits: Vec<EditRecord>,
    /// The night id (the backup directory).
    pub night: NightId,
    /// `candidate` (default): apply in memory, answer the text, write nothing.
    /// `record`: gate from tonight's `replay_case` rows of that text, then write.
    #[serde(default)]
    pub pass: Option<String>,
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
        "Dreaming only: apply ≤ 4 bounded edits (add | replace | delete with bullet-text anchors; retire for a \
         dreaming-made skill) to a skill's learned sidecar `skills/learned/<skill>.md`. `pass: candidate` \
         (default) applies in memory and returns the candidate `text` for `replay_case`; `pass: record` \
         recomputes the gate from tonight's replays of exactly that text — refusing when none were logged — \
         and writes the edits only when it is accepted (backup first). Returns the apply report \
         (applied | skipped_unmatched | error per edit) and the text. Never touches the skill itself."
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
        let pass = match a.pass.as_deref().map(str::trim) {
            None | Some("") | Some("candidate") => crate::edits::Pass::Candidate,
            Some("record") => crate::edits::Pass::Record,
            Some(other) => bail!("pass {other:?}: candidate | record"),
        };
        let r = crate::edits::apply(
            &self.0.data_dir,
            &self.0.cfg,
            &a.skill,
            &a.edits,
            &a.night,
            pass,
        )?;
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
    /// `provider/model` for the answer; `None` = the resolved `target` tier (`agent.model`).
    /// The judge is the resolved `judge` tier (136 D12).
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
         judge. Returns {answer, score (0–1), reason} and logs the score to `dreaming/replays.jsonl`, which \
         `skill_learned_edit(pass=\"record\")` gates on. Spends model budget."
            .into()
    }

    async fn call(&self, a: ReplayArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = crate::replay::replay(
            &self.0.data_dir,
            &self.0.cfg,
            &self.0.agent,
            cx.session,
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
    /// The line (≤ 140 chars); for `replace`: `"<old line> => <new line>"`.
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
         chars, in the system prompt every turn). Refuses past the cap. With `preferences.auto_from_explicit` \
         off, `add` files a `remember` proposal for the inbox instead of writing. Returns the block text."
            .into()
    }

    async fn call(&self, a: PreferencesSetArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.mutating(Self::NAME, &cx).await?;
        // The live switch (`SetSettings` updates the actor's config, not the seams').
        let actor = ActorRef::<Night>::lookup(DREAMING_ACTOR).ok().flatten();
        let auto = match &actor {
            Some(n) => n
                .ask(crate::night::Settings)
                .await
                .map_or(self.0.cfg.preferences.auto_from_explicit, |s| {
                    s.preferences_auto
                }),
            None => self.0.cfg.preferences.auto_from_explicit,
        };
        if a.op.trim() == "add" && !auto {
            // R-16 (D8): the user's veto on the always-on rung.
            let line = crate::scrub::clip(&crate::scrub::scrub(a.line.trim()), 140);
            anyhow::ensure!(!line.is_empty(), "empty line");
            let night = crate::ledger::tonight();
            let draft = ProposalDraft {
                kind: crate::types::ProposalKind::Remember,
                title: format!("I'll remember: {line}"),
                body: format!(
                    "A standing preference dreaming wanted to add to the system prompt block; \
                     `preferences.auto_from_explicit` is off, so it waits for you. Evidence: {}.",
                    if a.evidence.is_empty() {
                        "none recorded".to_owned()
                    } else {
                        a.evidence.join(", ")
                    }
                ),
                options: vec![
                    "confirm".into(),
                    "not that".into(),
                    "never save this".into(),
                ],
            };
            let rows = match actor {
                Some(actor) => actor
                    .ask(crate::night::Propose {
                        night: night.clone(),
                        drafts: vec![draft],
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?,
                None => {
                    crate::night::add_proposals(&self.0.data_dir, &self.0.cfg, &night, vec![draft])?
                }
            };
            return Ok(ToolOutput::ok(match rows.first() {
                Some(p) => format!(
                    "auto-promote is off: filed as a remember proposal `proposal:{}` (the block is unchanged)",
                    p.id
                ),
                None => "auto-promote is off and the weekly proposal cap is reached: not filed (the block is unchanged)".to_owned(),
            }));
        }
        let block = crate::preferences::set(
            &self.0.data_dir,
            self.0.knowledge.as_deref(),
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
         changed groups with their undo handles, spend). Three sections are special: `proposals` takes a JSON \
         array of {kind: routine|double_class|remember, title, body, options} and files them for the inbox \
         (≤ 3 per week); `meta-note` replaces the ≤ 1,200-char meta-note; `journal` closes the night \
         (counts, spend, last_dream_at) — call it last, once."
            .into()
    }

    async fn call(&self, a: JournalArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.mutating(Self::NAME, &cx).await?;
        let actor = ActorRef::<Night>::lookup(DREAMING_ACTOR).ok().flatten();
        match a.section.as_str() {
            "proposals" => {
                let drafts: Vec<ProposalDraft> =
                    serde_json::from_str(a.markdown.trim()).map_err(|e| {
                        anyhow::anyhow!("`proposals` takes a JSON array of drafts: {e}")
                    })?;
                let n = drafts.len();
                let actor = actor.ok_or_else(|| anyhow::anyhow!("dreaming is not running"))?;
                let rows = actor
                    .ask(crate::night::Propose {
                        night: a.night.clone(),
                        drafts,
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let mut md: String = rows
                    .iter()
                    .map(|p| {
                        format!(
                            "- {} — {} · `proposal:{}`\n",
                            p.title,
                            p.options.join(" / "),
                            p.id
                        )
                    })
                    .collect();
                if rows.len() < n {
                    md.push_str(&format!(
                        "- {} more not filed: the cap is {} rows per 7 days\n",
                        n - rows.len(),
                        self.0.cfg.caps.rows_per_week
                    ));
                }
                crate::journal::append(&self.0.data_dir, &a.night, "Proposals", &md).await?;
                Ok(structured(
                    format!("{} of {} proposals filed", rows.len(), n),
                    rows,
                ))
            }
            "meta-note" => {
                let actor = actor.ok_or_else(|| anyhow::anyhow!("dreaming is not running"))?;
                let old = actor
                    .ask(crate::night::SetMetaNote(a.markdown.clone()))
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let md = format!(
                    "Before:\n{}\n\nAfter:\n{}\n",
                    if old.trim().is_empty() {
                        "(empty)"
                    } else {
                        old.trim()
                    },
                    a.markdown.trim()
                );
                crate::journal::append(&self.0.data_dir, &a.night, "Meta-note", &md).await?;
                Ok(ToolOutput::ok("meta-note replaced"))
            }
            section => {
                crate::journal::append(&self.0.data_dir, &a.night, section, &a.markdown).await?;
                if let Some(actor) = actor {
                    let phase = section.to_lowercase();
                    actor
                        .ask(crate::night::Commit {
                            night: a.night.clone(),
                            phase: phase.clone(),
                            // P2 D3: `Skipped` is parsed line by line by the close gate.
                            summary: if phase == "skipped" {
                                a.markdown.clone()
                            } else {
                                a.markdown.lines().next().unwrap_or("").to_owned()
                            },
                        })
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    if phase == "journal" {
                        return Ok(ToolOutput::ok(format!("journal {}: night closed", a.night)));
                    }
                }
                Ok(ToolOutput::ok(format!(
                    "journal {}: {}",
                    a.night, a.section
                )))
            }
        }
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
        let r = match ActorRef::<Night>::lookup(DREAMING_ACTOR) {
            Ok(Some(actor)) => actor
                .ask(crate::night::Undo {
                    night: a.night.clone(),
                    group: a.group.clone(),
                })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
            _ => {
                crate::undo::undo(
                    &self.0.data_dir,
                    self.0.knowledge.as_deref(),
                    None,
                    &a.night,
                    a.group.as_deref(),
                )
                .await?
            }
        };
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
        for r in [StateTool::RISK, CaseListTool::RISK, ReplayTool::RISK] {
            assert_eq!(r, Risk::ReadOnly);
        }
        for r in [
            LedgerTool::RISK,
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
            agent: AgentDefaults::default(),
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
        assert!(e.contains("no case x"), "{e}");
    }

    /// R-10 (D9): `dreaming_ledger` is gated — a session that may not
    /// consolidate neither opens a night nor consumes the pending rewinds.
    #[tokio::test]
    async fn ledger_refuses_an_ungated_session_and_leaves_the_pending_rewinds() {
        let dir = tempfile::tempdir().unwrap();
        let seams = Arc::new(Seams {
            data_dir: dir.path().to_owned(),
            cfg: DreamConfig::default(),
            agent: AgentDefaults::default(),
            knowledge: None,
        });
        let pending = crate::ledger::pending_dir(dir.path()).join("a.json");
        crate::night::write_atomic(
            &pending,
            serde_json::to_string(&agent::LedgerRewind {
                session: runtime::SessionId::new(),
                at: "2026-10-07T21:05:00Z".into(),
                dropped_assistant_text: "tail".into(),
            })
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
        let e = LedgerTool(seams)
            .call(LedgerArgs::default(), cx(false))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("dreaming_ledger runs only"), "{e}");
        assert!(pending.exists(), "the rewind snapshot was not consumed");
        assert!(!crate::ledger::path(dir.path(), &crate::ledger::tonight()).exists());
        assert!(crate::night::read_record(dir.path(), &crate::ledger::tonight()).is_none());
    }

    /// R-16 (D8): with `preferences.auto_from_explicit` off, `add` files a
    /// `remember` proposal and leaves the block alone; `demote`/`replace`
    /// are unaffected.
    #[tokio::test]
    async fn preferences_add_becomes_a_proposal_when_auto_promote_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mut cfg = DreamConfig::default();
        cfg.preferences.auto_from_explicit = false;
        // A gate-passing context: the knowledge store with this session registered.
        let rt = runtime::Runtime::new(figment::Figment::new(), d.to_owned());
        let k = knowledge::Knowledge::open(d, rt).await.unwrap();
        let cx = cx(false);
        k.register_consolidating_session(cx.session);
        let seams = Arc::new(Seams {
            data_dir: d.to_owned(),
            cfg,
            agent: AgentDefaults::default(),
            knowledge: Some(k.clone()),
        });
        let out = PreferencesSetTool(seams.clone())
            .call(
                PreferencesSetArgs {
                    op: "add".into(),
                    line: "never use ask_user; numbered text".into(),
                    evidence: vec!["s1".into(), "s2".into()],
                },
                cx.clone(),
            )
            .await
            .unwrap();
        assert!(
            out.content
                .starts_with("auto-promote is off: filed as a remember proposal"),
            "{}",
            out.content
        );
        assert!(!crate::preferences::path(d).exists());
        let rows: Vec<crate::types::Proposal> =
            std::fs::read_to_string(crate::night::proposals_path(d))
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, crate::types::ProposalKind::Remember);
        assert_eq!(
            rows[0].title,
            "I'll remember: never use ask_user; numbered text"
        );
        // The switch on: the same call writes the block.
        let seams = Arc::new(Seams {
            data_dir: d.to_owned(),
            cfg: DreamConfig::default(),
            agent: AgentDefaults::default(),
            knowledge: Some(k),
        });
        let out = PreferencesSetTool(seams)
            .call(
                PreferencesSetArgs {
                    op: "add".into(),
                    line: "never use ask_user; numbered text".into(),
                    evidence: vec![],
                },
                cx,
            )
            .await
            .unwrap();
        assert!(
            out.content.contains("- never use ask_user; numbered text"),
            "{}",
            out.content
        );
        assert!(crate::preferences::path(d).exists());
    }
}
