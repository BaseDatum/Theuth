//! The `agent_propose` and `agents_list` tools (67 §6.5, §8), registered by
//! `RosterPlugin::start` once the tool registry is up.

use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::desk::Shared;
use crate::types::{AgentFile, AgentName, AgentStatus, Budget};
use crate::{ListAgents, ProposeAgent};

/// Register the roster tools on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>, shared: Arc<Shared>) {
    let _ = reg
        .ask(RegisterTool(Arc::new(AgentProposeTool(shared))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(AgentsListTool))).await;
}

/// Arguments for [`AgentProposeTool`].
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
    /// `none` (default) or `<amount> <CUR>/month`.
    #[serde(default)]
    pub budget: Option<String>,
    /// Risk ceiling for every session it opens: `read_only` | `mutating` | `destructive`.
    #[serde(default)]
    pub tools: Option<String>,
    /// MCP servers (or `server.tool`) it may see; empty = every server.
    #[serde(default)]
    pub mcp: Vec<String>,
    /// One paragraph on when this agent is the right one to hand off to.
    #[serde(default)]
    pub capabilities: Option<String>,
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
        let budget: Budget = match a.budget.as_deref().map(str::parse) {
            Some(Ok(b)) => b,
            Some(Err(e)) => return Ok(ToolOutput::err(e)),
            None => Budget::None,
        };
        let file = AgentFile {
            title: a.title.trim().to_owned(),
            name: a.name.clone(),
            reports_to: a
                .reports_to
                .map(|r| r.trim().to_owned())
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| proposer.clone()),
            model: None,
            effort: None,
            tools: a.tools,
            mcp: a.mcp,
            budget,
            capabilities: a.capabilities,
            paused: true,
            proposed_by: None,
            body: a.charter,
        };
        match roster
            .ask(ProposeAgent {
                file,
                proposed_by: format!("agent:{proposer}"),
            })
            .await
        {
            Ok(approval) => Ok(ToolOutput::ok(format!(
                "proposed agents/{}.md (paused until approved); approval {approval}",
                a.name
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
        for a in list.into_iter().filter(|s| !s.file.name.is_default()).filter(|s| {
            a.all
                || matches!(
                    s.status,
                    AgentStatus::Idle | AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Unreachable
                )
        }) {
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
                s.push_str(&format!(" budget: {:.2}/{amount} {currency}", a.spent_month));
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
