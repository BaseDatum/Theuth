//! `ToolRegistry` actor, registered as [`crate::REGISTRY_ACTOR`].
//! Owns every [`DynTool`]; every invocation passes through the one approval
//! contract here before the tool runs: `Risk → PolicyAction` from
//! `tools.policy`, with `Ask` rewritten to `Allow` by the approvals switches
//! and nothing else (12 §5).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::approval::{Approval, Decision, RequestApproval};
use crate::{
    APPROVAL_ACTOR, DynTool, Policy, PolicyAction, REGISTRY_ACTOR, ToolCall, ToolCx, ToolOutput,
    ToolSpec, ToolsConfig, fs, jobs, shell,
};

/// The registry actor. Look up with `ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)`.
/// `policy` is the only permission input; a tool's `ToolSpec.risk` is the
/// only key into it (12 §5).
#[derive(kameo::Actor)]
pub struct ToolRegistry {
    tools: HashMap<String, DynTool>,
    /// Plugin system-prompt blocks by plugin name (07 §4 part 7): sorted, so
    /// the rendered prompt is stable across restarts.
    prompts: BTreeMap<&'static str, String>,
    policy: Policy,
    /// `tools.approvals`: false turns every `Ask` into `Allow`.
    approvals: bool,
    /// Sessions that turned prompts off for themselves (and, through
    /// `parents`, for their subagents).
    bypass: HashSet<SessionId>,
    /// child → parent, from [`SetParent`]; a session inherits its ancestors' bypass.
    parents: HashMap<SessionId, SessionId>,
    /// `<data_dir>/config.yaml`, for the persistent switch.
    config_path: PathBuf,
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

/// `ask` → `()`. Removes a tool by name; unknown name is a no-op.
pub struct UnregisterTool(pub String);

/// `ask` → `()`. A plugin's system-prompt paragraph: what its tools are for and
/// when to reach for them. Pure function of config (no clock, session, roster):
/// anything dynamic busts the system cache. Replaces the block for `plugin`.
pub struct RegisterPrompt {
    /// The plugin's `Plugin::name`.
    pub plugin: &'static str,
    /// Plain sentences; the harness owns headings and order.
    pub text: String,
}

/// `ask` → `()`. Removes a plugin's prompt block; unknown plugin is a no-op.
pub struct UnregisterPrompt(pub &'static str);

/// `ask` → every registered block, sorted by plugin name.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListPrompts;

/// `ask` → [`ApprovalsState`]. Turn approval prompts on or off: for one
/// session (in memory) or, with `session: None`, for everyone, persisted to
/// `tools.approvals` in `<data_dir>/config.yaml`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetApprovals {
    /// Session to switch; `None` switches the global default.
    pub session: Option<SessionId>,
    /// `true` prompts as the policy says; `false` never prompts.
    pub enabled: bool,
}

/// `tell`: record that `session` is a subagent of `parent`, so it inherits
/// the parent's approval switch.
#[derive(Clone, Copy, Debug)]
pub struct SetParent {
    /// Child session.
    pub session: SessionId,
    /// Its parent.
    pub parent: SessionId,
}

impl Message<SetParent> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetParent, _: &mut Context<Self, Self::Reply>) {
        self.parents.insert(m.session, m.parent);
    }
}

/// Whether prompts are on, globally and for the session asked about.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ApprovalsState {
    /// `tools.approvals`.
    pub global: bool,
    /// False when the session opted out; meaningful only for a session query.
    pub session: bool,
}

impl Message<SetApprovals> for ToolRegistry {
    type Reply = ApprovalsState;

    async fn handle(
        &mut self,
        m: SetApprovals,
        _: &mut Context<Self, Self::Reply>,
    ) -> ApprovalsState {
        match m.session {
            Some(id) if m.enabled => {
                self.bypass.remove(&id);
            }
            Some(id) => {
                self.bypass.insert(id);
            }
            None => {
                self.approvals = m.enabled;
                let value = if m.enabled { "true" } else { "false" };
                if let Err(e) =
                    runtime::write_config_value(&self.config_path, "tools", "approvals", value)
                {
                    tracing::warn!("config.yaml: {e}");
                }
            }
        }
        ApprovalsState {
            global: self.approvals,
            session: m.session.is_none_or(|id| !self.bypassed(id)),
        }
    }
}

impl ToolRegistry {
    /// The session or any ancestor opted out of prompts.
    fn bypassed(&self, mut id: SessionId) -> bool {
        // ponytail: linear walk per invoke; depth is capped by `max_depth`.
        loop {
            if self.bypass.contains(&id) {
                return true;
            }
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return false,
            }
        }
    }
}

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

impl Message<UnregisterTool> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: UnregisterTool, _: &mut Context<Self, Self::Reply>) {
        if self.tools.remove(&m.0).is_some() {
            tracing::info!(tool = %m.0, "unregistered tool");
        }
    }
}

impl Message<RegisterPrompt> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: RegisterPrompt, _: &mut Context<Self, Self::Reply>) {
        tracing::info!(
            plugin = m.plugin,
            chars = m.text.len(),
            "registered prompt block"
        );
        self.prompts.insert(m.plugin, m.text);
    }
}

impl Message<UnregisterPrompt> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: UnregisterPrompt, _: &mut Context<Self, Self::Reply>) {
        if self.prompts.remove(m.0).is_some() {
            tracing::info!(plugin = m.0, "unregistered prompt block");
        }
    }
}

impl Message<ListPrompts> for ToolRegistry {
    type Reply = Vec<String>;

    async fn handle(&mut self, _: ListPrompts, _: &mut Context<Self, Self::Reply>) -> Vec<String> {
        self.prompts.values().cloned().collect()
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
        let mut action = self.policy.action(spec.risk);
        if action == PolicyAction::Ask && (!self.approvals || self.bypassed(call.session)) {
            action = PolicyAction::Allow;
        }
        let approval = match action {
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
            args: call.args.clone(),
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

/// The `tools` section, defaults when absent.
pub(crate) fn config(rt: &Runtime) -> anyhow::Result<ToolsConfig> {
    if rt.config.contains("tools") {
        rt.section("tools")
    } else {
        Ok(ToolsConfig::default())
    }
}

/// Spawn registry, approval and jobs actors and register builtins. Returns the
/// registry so the caller can attach further tool sources (MCP).
pub async fn start(rt: &Runtime) -> anyhow::Result<ActorRef<ToolRegistry>> {
    let cfg = config(rt)?;

    let approval = Approval::spawn_with_mailbox(
        Approval::new(rt.clone(), Duration::from_secs(cfg.approval_timeout_secs)),
        runtime::subscriber_mailbox(),
    );
    approval.register(APPROVAL_ACTOR)?;
    let jobs = jobs::start(
        rt,
        Duration::from_secs(cfg.job_grace_secs),
        Duration::from_secs(cfg.job_timeout_secs),
    )?;

    let builtins: [DynTool; 8] = [
        Arc::new(shell::ShellTool::new(jobs.clone())),
        Arc::new(jobs::JobsTool(jobs.clone())),
        Arc::new(jobs::CancelJobTool(jobs)),
        Arc::new(fs::ReadFile),
        Arc::new(fs::WriteFile),
        Arc::new(fs::ListDir),
        Arc::new(fs::Grep),
        Arc::new(fs::Edit),
    ];
    let registry = ToolRegistry::spawn_with_mailbox(
        ToolRegistry {
            tools: builtins.into_iter().map(|t| (t.spec().name, t)).collect(),
            prompts: BTreeMap::new(),
            policy: cfg.policy,
            approvals: cfg.approvals,
            bypass: HashSet::new(),
            parents: HashMap::new(),
            config_path: rt.data_dir.join("config.yaml"),
            approval: None,
        },
        runtime::subscriber_mailbox(),
    );
    registry.register(REGISTRY_ACTOR)?;
    Ok(registry)
}
