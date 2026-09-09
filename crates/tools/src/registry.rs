//! `ToolRegistry` actor, registered as [`crate::REGISTRY_ACTOR`].
//! Owns every [`DynTool`]; every invocation passes through the approval
//! policy here before the tool runs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::Runtime;

use crate::approval::{Approval, Decision, RequestApproval};
use crate::{
    APPROVAL_ACTOR, DynTool, Policy, PolicyAction, REGISTRY_ACTOR, ToolCall, ToolCx, ToolOutput,
    ToolSpec, ToolsConfig, fs, mcp, shell,
};

/// The registry actor. Look up with `ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)`.
#[derive(kameo::Actor)]
pub struct ToolRegistry {
    tools: HashMap<String, DynTool>,
    policy: Policy,
    /// Looked up at first `Ask`, never at start (no start-order coupling).
    approval: Option<ActorRef<Approval>>,
}

/// `ask` → `Vec<ToolSpec>` for every registered tool, sorted by name.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListTools;

/// `ask` → [`ToolOutput`]. Never fails: unknown tool, denied approval, bad
/// args, tool panic and timeout all come back as `ToolOutput { is_error: true }`.
#[derive(Debug)]
pub struct Invoke(pub ToolCall);

/// `ask` → `()`. Replaces an existing tool with the same name.
pub struct RegisterTool(pub DynTool);

impl Message<ListTools> for ToolRegistry {
    type Reply = Vec<ToolSpec>;

    async fn handle(&mut self, _: ListTools, _: &mut Context<Self, Self::Reply>) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self.tools.values().map(|t| t.spec()).collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }
}

impl Message<RegisterTool> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: RegisterTool, _: &mut Context<Self, Self::Reply>) {
        let name = m.0.spec().name;
        tracing::info!(tool = %name, "registered tool");
        self.tools.insert(name, m.0);
    }
}

impl Message<Invoke> for ToolRegistry {
    type Reply = DelegatedReply<ToolOutput>;

    async fn handle(
        &mut self,
        Invoke(call): Invoke,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let Some(tool) = self.tools.get(&call.name).cloned() else {
            tx.send(ToolOutput::err(format!("unknown tool: {}", call.name)));
            return reply;
        };
        let spec = tool.spec();
        let approval = match self.policy.action(spec.risk) {
            PolicyAction::Allow => None,
            PolicyAction::Deny => {
                tx.send(ToolOutput::err(format!("{}: denied by policy", call.name)));
                return reply;
            }
            PolicyAction::Ask => {
                if self.approval.is_none() {
                    self.approval = ActorRef::<Approval>::lookup(APPROVAL_ACTOR).ok().flatten();
                }
                let Some(a) = self.approval.clone() else {
                    tx.send(ToolOutput::err(format!(
                        "{}: approval required but no approval actor is running",
                        call.name
                    )));
                    return reply;
                };
                Some(a)
            }
        };
        // Everything that awaits leaves the mailbox.
        tokio::spawn(async move {
            tx.send(invoke(tool, spec, call, approval).await);
        });
        reply
    }
}

async fn invoke(
    tool: DynTool,
    spec: ToolSpec,
    call: ToolCall,
    approval: Option<ActorRef<Approval>>,
) -> ToolOutput {
    if let Some(approval) = approval {
        let req = RequestApproval {
            session: call.session,
            tool: spec.name.clone(),
            summary: tool.summary(&call.args),
            risk: spec.risk,
        };
        match approval.ask(req).await {
            Ok(r) if r.decision == Decision::Allow => {}
            Ok(r) if r.timed_out => {
                return ToolOutput::err(format!("{}: approval timed out", spec.name));
            }
            Ok(_) => return ToolOutput::err(format!("{}: denied by user", spec.name)),
            Err(e) => return ToolOutput::err(format!("{}: approval unavailable: {e}", spec.name)),
        }
    }
    let cx = ToolCx {
        session: call.session,
        call_id: call.id,
    };
    // Own task so a panicking tool is an error output, not a dead registry.
    match tokio::spawn(async move { tool.call(call.args, cx).await }).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => ToolOutput::err(format!("{}: {e:#}", spec.name)),
        Err(e) => ToolOutput::err(format!("{}: tool panicked: {e}", spec.name)),
    }
}

/// Spawn registry + approval actors, register builtins and MCP servers.
pub async fn start(rt: &Runtime) -> anyhow::Result<()> {
    let cfg: ToolsConfig = if rt.config.contains("tools") {
        rt.section("tools")?
    } else {
        ToolsConfig::default()
    };

    let approval = Approval::spawn_with_mailbox(
        Approval::new(rt.clone(), Duration::from_secs(cfg.approval_timeout_secs)),
        runtime::subscriber_mailbox(),
    );
    approval.register(APPROVAL_ACTOR)?;

    let builtins: [DynTool; 4] = [
        Arc::new(shell::ShellTool::new(Duration::from_secs(
            cfg.shell_timeout_secs,
        ))),
        Arc::new(fs::ReadFile),
        Arc::new(fs::WriteFile),
        Arc::new(fs::ListDir),
    ];
    let registry = ToolRegistry::spawn_with_mailbox(
        ToolRegistry {
            tools: builtins.into_iter().map(|t| (t.spec().name, t)).collect(),
            policy: cfg.policy,
            approval: None,
        },
        runtime::subscriber_mailbox(),
    );
    registry.register(REGISTRY_ACTOR)?;

    for server in cfg.mcp {
        let registry = registry.clone();
        let name = server.name.clone();
        tokio::spawn(async move {
            if let Err(e) = mcp::connect(server, registry).await {
                tracing::error!(server = %name, error = %e, "mcp server failed");
            }
        });
    }
    Ok(())
}
