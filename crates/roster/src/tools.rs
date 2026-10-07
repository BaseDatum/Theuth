//! The `agent_propose`, `agent_put` and `agents_list` tools (67 §6.5, §8),
//! registered by `RosterPlugin::start` once the tool registry is up.

use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::desk::Shared;
use crate::types::{AgentFile, AgentName, AgentStatus, Budget};
use crate::{GetAgent, ListAgents, ProposeAgent, PutAgent};

/// The roster's system-prompt paragraph (`RegisterPrompt { plugin: "roster" }`,
/// 70 AGENTS-CHAT): hires are proposed in chat as cards; the user approves.
pub const PROMPT: &str = "## Hiring\n\
Agents are files (`agents/<name>.md`), but the user never writes one: hiring happens in chat. When the user \
describes a standing role, asks to hire or add an agent, or a role is plainly missing, call `agents_list()`, then \
`agent_draft()` with the proposed agents (name `[a-z0-9-]{1,32}`, title, `reports_to` = `theuth` or an existing \
agent, charter a fresh model can act on alone, plus model / effort / tools / mcp / budget / capabilities as \
needed) and wait: the user's press arrives as their next message (`[app agent-drafts …]` naming a draft id and \
`hire | hire paused | replace | discard`). Then `agent_put()` with that draft's fields — `paused: false` for hire \
or replace, `paused: true` for hire paused — and nothing for discard. Never tell the user to write or edit a \
file. Desks (agent sessions) do not `agent_put()`: they `agent_propose()`, which lands the agent paused and files a \
`hire_agent` approval up their line; the user decides in the Agents view.";

/// Register the roster tools on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>, shared: Arc<Shared>) {
    let _ = reg
        .ask(RegisterTool(Arc::new(AgentProposeTool(shared.clone()))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(AgentPutTool(shared)))).await;
    let _ = reg.ask(RegisterTool(Arc::new(AgentsListTool))).await;
}

/// Arguments for [`AgentProposeTool`]; also the agent part of [`AgentPutArgs`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgentProposeArgs {
    /// `[a-z0-9-]{1,32}`; becomes `agents/<name>.md`.
    pub name: AgentName,
    /// Display title.
    pub title: String,
    /// Manager: an agent name, or `theuth`. Default: the proposing agent.
    #[serde(default)]
    pub reports_to: Option<String>,
    /// The charter (markdown): standing instructions, what it owns, what it never does.
    pub charter: String,
    /// Model for every session it opens; default: the harness default.
    #[serde(default)]
    pub model: Option<String>,
    /// Effort for every session it opens.
    #[serde(default)]
    pub effort: Option<String>,
    /// `none` (default) or `<amount> <CUR>/month`.
    #[serde(default)]
    pub budget: Option<String>,
    /// Risk ceiling for every session it opens: `read_only` | `mutating` | `destructive`.
    #[serde(default)]
    pub tools: Option<String>,
    /// MCP servers (or `server.tool`) it may see; omit = every server, `[]` = none.
    #[serde(default)]
    pub mcp: Option<Vec<String>>,
    /// One paragraph on when this agent is the right one to hand off to.
    #[serde(default)]
    pub capabilities: Option<String>,
}

/// The file `a` describes, active and unproposed (the caller sets `paused` /
/// `proposed_by`). `Err`: a bad budget, or a `reports_to` that is neither
/// `theuth` nor an agent on the roster.
fn file_of(
    a: AgentProposeArgs,
    default_reports_to: &str,
    shared: &Shared,
) -> Result<AgentFile, String> {
    let budget: Budget = match a.budget.as_deref().map(str::parse) {
        Some(Ok(b)) => b,
        Some(Err(e)) => return Err(e),
        None => Budget::None,
    };
    let reports_to = a
        .reports_to
        .map(|r| r.trim().to_owned())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| default_reports_to.to_owned());
    if reports_to != AgentName::DEFAULT && !shared.files().contains_key(&reports_to) {
        return Err(format!(
            "reports_to {reports_to:?}: no such agent (see agents_list); use `theuth` or an existing name"
        ));
    }
    Ok(AgentFile {
        name: a.name,
        title: a.title.trim().to_owned(),
        reports_to,
        model: a.model,
        effort: a.effort,
        tools: a.tools,
        mcp: a.mcp,
        budget,
        capabilities: a.capabilities,
        paused: false,
        proposed_by: None,
        runs_in: Default::default(),
        body: a.charter,
    })
}

/// `agent_propose`: write a paused agent file and file a `hire_agent` approval (67 §6.5).
pub struct AgentProposeTool(pub Arc<Shared>);

#[async_trait::async_trait]
impl Tool for AgentProposeTool {
    const NAME: &'static str = "agent_propose";
    const RISK: Risk = Risk::Mutating;
    type Args = AgentProposeArgs;

    fn description(&self) -> String {
        "Propose a new agent. Writes `agents/<name>.md` with `paused: true` and `proposed_by: agent:<you>`, \
         then files a `hire_agent` approval up your reporting line, ending at the user; the agent receives \
         no tasks until someone approves it, which flips `paused`. Use it when a standing role is missing \
         from the roster (call `agents_list` first) — not for one piece of work, which is a task. Give a \
         charter that a fresh model can act on alone: purpose, what it owns, what it never does, how it \
         reports. Returns the file path and the approval id."
            .into()
    }

    async fn call(&self, a: AgentProposeArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(roster) = crate::actor() else {
            return Ok(ToolOutput::err("roster is not running"));
        };
        let proposer = self
            .0
            .agent_of(cx.session)
            .unwrap_or_else(|| AgentName::DEFAULT.to_owned());
        let name = a.name.clone();
        let mut file = match file_of(a, &proposer, &self.0) {
            Ok(f) => f,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        file.paused = true;
        match roster
            .ask(ProposeAgent {
                file,
                proposed_by: format!("agent:{proposer}"),
            })
            .await
        {
            Ok(approval) => Ok(ToolOutput::ok(format!(
                "proposed agents/{name}.md (paused until approved); approval {approval}"
            ))),
            Err(e) => Ok(ToolOutput::err(e.to_string())),
        }
    }
}

/// Arguments for [`AgentPutTool`]: the agent, plus `paused`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgentPutArgs {
    /// The agent.
    #[serde(flatten)]
    pub agent: AgentProposeArgs,
    /// `true`: hired on hold — takes no wakes until un-paused (Agents view / `agent_pause`).
    #[serde(default)]
    pub paused: bool,
}

/// `agent_put`: write `agents/<name>.md` on the user's approval (create or
/// replace). Only the user's own sessions and theuth's desk may call it;
/// a desk proposes with `agent_propose`.
pub struct AgentPutTool(pub Arc<Shared>);

#[async_trait::async_trait]
impl Tool for AgentPutTool {
    const NAME: &'static str = "agent_put";
    const RISK: Risk = Risk::Mutating;
    type Args = AgentPutArgs;

    fn description(&self) -> String {
        "Hire (or replace) an agent the user approved: writes `agents/<name>.md` with the given fields and \
         `paused: false | true`, active at once (the press on an `agent_draft` card is the approval, so no \
         `proposed_by`). Call it only after the user chose Hire / Hire paused / Replace on a draft card, with \
         that draft's fields; an existing name is overwritten. Desks may not call it: they `agent_propose` and \
         the user decides. `reports_to` must be `theuth` or an agent on the roster."
            .into()
    }

    async fn call(&self, a: AgentPutArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(roster) = crate::actor() else {
            return Ok(ToolOutput::err("roster is not running"));
        };
        if let Some(desk) = self
            .0
            .agent_of(cx.session)
            .filter(|d| d != AgentName::DEFAULT)
        {
            return Ok(ToolOutput::err(format!(
                "agent_put is the user's approval; as agent {desk}'s desk, propose with agent_propose"
            )));
        }
        let mut file = match file_of(a.agent, AgentName::DEFAULT, &self.0) {
            Ok(f) => f,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        file.paused = a.paused;
        let name = file.name.clone();
        let existed = roster.ask(GetAgent(name.to_string())).await.is_ok();
        match roster.ask(PutAgent(file)).await {
            Ok(()) => Ok(ToolOutput::ok(format!(
                "{} agents/{name}.md ({})",
                if existed { "replaced" } else { "hired" },
                if a.paused {
                    "paused: un-pause from the Agents view when ready"
                } else {
                    "active"
                }
            ))),
            Err(e) => Ok(ToolOutput::err(e.to_string())),
        }
    }
}

/// Arguments for [`AgentsListTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentsListArgs {
    /// `true`: include paused, proposed and invalid agents (default: only ones that take work).
    #[serde(default)]
    pub all: bool,
}

/// `agents_list`: the roster, for hand-offs and delegation.
pub struct AgentsListTool;

#[async_trait::async_trait]
impl Tool for AgentsListTool {
    const NAME: &'static str = "agents_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = AgentsListArgs;

    fn description(&self) -> String {
        "List the agents on the roster: one line per agent with name, title, who it reports to, its \
         `capabilities` paragraph (when it is the right one to hand work to), status (idle / working / \
         blocked / paused / proposed / unreachable), open task count and budget use. `theuth` is the \
         built-in default agent and is not listed: anything unaddressed is theirs. Read it before \
         `task_create` with another owner, or before `agent_propose`, so you do not duplicate a role."
            .into()
    }

    async fn call(&self, a: AgentsListArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(roster) = crate::actor() else {
            return Ok(ToolOutput::err("roster is not running"));
        };
        let list = match roster.ask(ListAgents).await {
            Ok(l) => l,
            Err(e) => return Ok(ToolOutput::err(e.to_string())),
        };
        let mut s = String::new();
        let mut rows = Vec::new();
        for a in list
            .into_iter()
            .filter(|s| !s.file.name.is_default())
            .filter(|s| {
                a.all
                    || matches!(
                        s.status,
                        AgentStatus::Idle
                            | AgentStatus::Working
                            | AgentStatus::Blocked
                            | AgentStatus::Unreachable
                    )
            })
        {
            let f = &a.file;
            s.push_str(&format!(
                "{} — {} (reports to {}) [{}] open tasks: {}",
                f.name,
                f.title,
                f.reports_to,
                serde_json::to_value(a.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                a.open_tasks
            ));
            if let Budget::Monthly { amount, currency } = &f.budget {
                s.push_str(&format!(
                    " budget: {:.2}/{amount} {currency}",
                    a.spent_month
                ));
            }
            if let Some(c) = &f.capabilities {
                s.push_str(&format!("\n  {c}"));
            }
            if let Some(e) = &a.error {
                s.push_str(&format!("\n  invalid: {e}"));
            }
            s.push('\n');
            rows.push(serde_json::json!({
                "name": f.name,
                "title": f.title,
                "reports_to": f.reports_to,
                "capabilities": f.capabilities,
                "status": a.status,
                "open_tasks": a.open_tasks,
                "error": a.error,
            }));
        }
        if s.is_empty() {
            s.push_str("no agents on the roster; theuth handles everything");
        }
        Ok(ToolOutput {
            structured: Some(serde_json::Value::Array(rows)),
            ..ToolOutput::ok(s)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::file;

    fn args(name: &str, reports_to: Option<&str>) -> AgentProposeArgs {
        AgentProposeArgs {
            name: AgentName::new(name).unwrap(),
            title: " Ops ".into(),
            reports_to: reports_to.map(str::to_owned),
            charter: "You are Ops.".into(),
            model: Some("gpt-5".into()),
            effort: None,
            budget: Some("40 USD/month".into()),
            tools: Some("mutating".into()),
            mcp: None,
            capabilities: None,
        }
    }

    #[test]
    fn file_of_checks_reports_to_and_keeps_the_fields() {
        let shared = Shared::default();
        let cto = file_of(args("cto", None), AgentName::DEFAULT, &shared).unwrap();
        assert_eq!(
            (
                cto.reports_to.as_str(),
                cto.title.as_str(),
                cto.model.as_deref()
            ),
            ("theuth", "Ops", Some("gpt-5"))
        );
        assert_eq!(
            cto.budget,
            Budget::Monthly {
                amount: 40.0,
                currency: "USD".into()
            }
        );
        assert!(!cto.paused && cto.proposed_by.is_none());
        // an unknown manager is refused; a roster agent or theuth is fine; blank falls back to the default
        assert!(
            file_of(args("ops", Some("cto")), AgentName::DEFAULT, &shared)
                .unwrap_err()
                .contains("no such agent")
        );
        shared.set_files(
            BTreeMap::from([("cto".to_owned(), Ok(cto))]),
            file::default_agent(std::path::Path::new("/nonexistent")),
        );
        assert_eq!(
            file_of(args("ops", Some(" cto ")), AgentName::DEFAULT, &shared)
                .unwrap()
                .reports_to,
            "cto"
        );
        assert_eq!(
            file_of(args("ops", Some("")), "cto", &shared)
                .unwrap()
                .reports_to,
            "cto"
        );
        assert!(file_of(args("ops", Some("theuth")), "cto", &shared).is_ok());
        assert!(
            file_of(
                AgentProposeArgs {
                    budget: Some("lots".into()),
                    ..args("ops", None)
                },
                "cto",
                &shared
            )
            .is_err()
        );
    }

    #[test]
    fn mcp_arg_omitted_is_all_and_empty_is_none() {
        let base = r#""name":"ops","title":"Ops","charter":"You are Ops.""#;
        let parse = |extra: &str| {
            serde_json::from_str::<AgentProposeArgs>(&format!("{{{base}{extra}}}"))
                .unwrap()
                .mcp
        };
        assert_eq!(parse(""), None);
        assert_eq!(parse(r#","mcp":[]"#), Some(vec![]));
        assert_eq!(
            parse(r#","mcp":["grafana"]"#),
            Some(vec!["grafana".to_owned()])
        );
    }

    #[test]
    fn hiring_block_reaches_theuth_and_the_desk_manual_points_at_propose() {
        let dir = std::env::temp_dir();
        let attended = agent::frame::baseline(&dir, None, "", PROMPT, false, None, true, false);
        let unattended = agent::frame::baseline(&dir, None, "", PROMPT, false, None, false, false);
        for p in [&attended, &unattended] {
            assert!(p.contains("## Hiring\n"), "{p}");
            assert!(
                p.contains("`agent_draft()`")
                    && p.contains("`agent_put()`")
                    && p.contains("Never tell the user to write or edit a file")
            );
        }
        let manual = crate::desk::manual(&file::default_agent(&dir), 3);
        assert!(manual.contains("`agent_propose()`") && !manual.contains("agent_put"));
    }
}
