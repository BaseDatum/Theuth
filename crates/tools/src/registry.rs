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
use crate::ask::{AskUser, AskUserTool};
use crate::{
    APPROVAL_ACTOR, ASK_ACTOR, DynTool, Policy, PolicyAction, REGISTRY_ACTOR, Risk, ToolCall,
    ToolCx, ToolOutput, ToolSpec, ToolsConfig, fs, jobs, pty, shell,
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
    /// Tool names a session (and its subagents) does not see: absent from
    /// [`ListTools`], `unknown tool` on [`Invoke`]. In memory only; the
    /// persistent switch is `off` in `mcp/config.yaml`.
    hidden: HashMap<SessionId, HashSet<String>>,
    /// child → parent, from [`SetParent`]; a session inherits its ancestors' bypass and hidden set.
    parents: HashMap<SessionId, SessionId>,
    /// `<data_dir>/config.yaml`, for the persistent switch.
    config_path: PathBuf,
    /// Looked up at first `Ask`, never at start (no start-order coupling).
    approval: Option<ActorRef<Approval>>,
}

/// `ask` → `Vec<ToolSpec>` for every registered tool, sorted by name; with a
/// session, minus what [`SetHidden`] hid for it or an ancestor.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListTools(pub Option<SessionId>);

/// `ask` → `()`. Hide (or show again) tools for one session, in memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetHidden {
    /// Session to switch (its subagents follow).
    pub session: SessionId,
    /// Registered tool names.
    pub tools: Vec<String>,
    /// `true` hides.
    pub hidden: bool,
}

/// `ask` → `HashSet<String>`: what [`SetHidden`] hid for the session or an ancestor.
#[derive(Clone, Copy, Debug)]
pub struct HiddenTools(pub SessionId);

/// `ask` → [`ToolOutput`]. Never fails: unknown tool, denied approval, bad
/// args, tool panic and timeout all come back as `ToolOutput { is_error: true }`.
#[derive(Debug)]
pub struct Invoke(pub ToolCall);

/// `ask` → `()`. Replaces an existing tool with the same name.
pub struct RegisterTool(pub DynTool);

/// `ask` → `()`. Removes a tool by name; unknown name is a no-op.
pub struct UnregisterTool(pub String);

/// `ask` → [`Decision`]: the same gate as [`Invoke`] for a tool call wth does
/// not run itself — an external harness's `session/request_permission`
/// (design doc 06 §3.4, §8.8). Timeout and a missing approval actor are `Deny`.
#[derive(Clone, Debug)]
pub struct ExternalApproval {
    /// Session the call belongs to (approval bypass inheritance).
    pub session: SessionId,
    /// Tool title, shown in the modal.
    pub tool: String,
    /// Raw arguments, shown in the modal.
    pub args: serde_json::Value,
    /// Risk class the caller derived (`acp::risk(kind)`).
    pub risk: Risk,
}

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

/// `ask` → [`ApprovalsState`]: the switches as they stand, for the global
/// default (`session: None`) or one session, without changing anything.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetApprovals {
    /// Session to read; `None` reads the global default alone.
    pub session: Option<SessionId>,
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
        self.approvals_state(m.session)
    }
}

impl Message<GetApprovals> for ToolRegistry {
    type Reply = ApprovalsState;

    async fn handle(
        &mut self,
        m: GetApprovals,
        _: &mut Context<Self, Self::Reply>,
    ) -> ApprovalsState {
        self.approvals_state(m.session)
    }
}

impl ToolRegistry {
    /// The global switch and, for a session, whether it (or an ancestor) opted out.
    fn approvals_state(&self, session: Option<SessionId>) -> ApprovalsState {
        ApprovalsState {
            global: self.approvals,
            session: session.is_none_or(|id| !self.bypassed(id)),
        }
    }

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

    /// `name` is hidden for the session or an ancestor.
    fn is_hidden(&self, mut id: SessionId, name: &str) -> bool {
        loop {
            if self.hidden.get(&id).is_some_and(|h| h.contains(name)) {
                return true;
            }
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return false,
            }
        }
    }

    /// The one permission decision (12 §5), shared by [`Invoke`] and
    /// [`ExternalApproval`]: `Ok(None)` run now, `Ok(Some(approval))` ask the
    /// human first, `Err(why)` refuse.
    fn gate(
        &mut self,
        session: SessionId,
        risk: Risk,
    ) -> Result<Option<ActorRef<Approval>>, &'static str> {
        let mut action = self.policy.action(risk);
        if action == PolicyAction::Ask && (!self.approvals || self.bypassed(session)) {
            action = PolicyAction::Allow;
        }
        match action {
            PolicyAction::Allow => Ok(None),
            PolicyAction::Deny => Err("denied by policy"),
            PolicyAction::Ask => {
                if self.approval.is_none() {
                    self.approval = ActorRef::<Approval>::lookup(APPROVAL_ACTOR).ok().flatten();
                }
                self.approval
                    .clone()
                    .map(Some)
                    .ok_or("approval required but no approval actor is running")
            }
        }
    }
}

impl Message<ExternalApproval> for ToolRegistry {
    type Reply = DelegatedReply<Decision>;

    async fn handle(
        &mut self,
        m: ExternalApproval,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let approval = match self.gate(m.session, m.risk) {
            Ok(None) => {
                tx.send(Decision::Allow);
                return reply;
            }
            Ok(Some(a)) => a,
            Err(why) => {
                tracing::info!(tool = %m.tool, "external approval: {why}");
                tx.send(Decision::Deny);
                return reply;
            }
        };
        tokio::spawn(async move {
            let req = RequestApproval {
                session: m.session,
                tool: m.tool,
                args: m.args,
                risk: m.risk,
            };
            // Timeout already resolves as `Deny`; an unreachable actor is one too.
            tx.send(
                approval
                    .ask(req)
                    .await
                    .map_or(Decision::Deny, |r| r.decision),
            );
        });
        reply
    }
}

impl Message<ListTools> for ToolRegistry {
    type Reply = Vec<ToolSpec>;

    async fn handle(
        &mut self,
        ListTools(session): ListTools,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .tools
            .iter()
            .filter(|(name, _)| !session.is_some_and(|s| self.is_hidden(s, name)))
            .map(|(_, t)| t.spec())
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }
}

impl Message<SetHidden> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetHidden, _: &mut Context<Self, Self::Reply>) {
        let set = self.hidden.entry(m.session).or_default();
        if m.hidden {
            set.extend(m.tools);
        } else {
            set.retain(|t| !m.tools.contains(t));
        }
        if set.is_empty() {
            self.hidden.remove(&m.session);
        }
    }
}

impl Message<HiddenTools> for ToolRegistry {
    type Reply = HashSet<String>;

    async fn handle(
        &mut self,
        HiddenTools(mut id): HiddenTools,
        _: &mut Context<Self, Self::Reply>,
    ) -> HashSet<String> {
        let mut out = HashSet::new();
        loop {
            if let Some(h) = self.hidden.get(&id) {
                out.extend(h.iter().cloned());
            }
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return out,
            }
        }
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
        let Some(tool) = self
            .tools
            .get(&call.name)
            .filter(|_| !self.is_hidden(call.session, &call.name))
            .cloned()
        else {
            tx.send(ToolOutput::err(format!("unknown tool: {}", call.name)));
            return reply;
        };
        let spec = tool.spec();
        let approval = match self.gate(call.session, spec.risk) {
            Ok(a) => a,
            Err(why) => {
                tx.send(ToolOutput::err(format!("{}: {why}", call.name)));
                return reply;
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
    let ask = AskUser::spawn_with_mailbox(
        AskUser::new(rt.clone(), Duration::from_secs(cfg.ask_timeout_secs)),
        runtime::subscriber_mailbox(),
    );
    ask.register(ASK_ACTOR)?;
    let jobs = jobs::start(
        rt,
        Duration::from_secs(cfg.job_grace_secs),
        Duration::from_secs(cfg.job_timeout_secs),
    )?;

    let shell = shell::ShellTool::new(jobs.clone());
    let shells = shell.shells();
    let builtins: [DynTool; 13] = [
        Arc::new(shell),
        Arc::new(pty::StartTool::new(
            jobs.clone(),
            shells,
            Duration::from_secs(cfg.pty_ready_timeout_secs),
        )),
        Arc::new(pty::SendTool(jobs.clone())),
        Arc::new(pty::ScreenTool(jobs.clone())),
        Arc::new(pty::WaitTool(jobs.clone())),
        Arc::new(jobs::JobsTool(jobs.clone())),
        Arc::new(jobs::CancelJobTool(jobs)),
        Arc::new(fs::ReadFile),
        Arc::new(fs::WriteFile),
        Arc::new(fs::ListDir),
        Arc::new(fs::Grep),
        Arc::new(fs::Edit),
        Arc::new(AskUserTool(ask)),
    ];
    let registry = ToolRegistry::spawn_with_mailbox(
        ToolRegistry {
            tools: builtins.into_iter().map(|t| (t.spec().name, t)).collect(),
            prompts: BTreeMap::new(),
            policy: cfg.policy,
            approvals: cfg.approvals,
            bypass: HashSet::new(),
            hidden: HashMap::new(),
            parents: HashMap::new(),
            config_path: rt.data_dir.join("config.yaml"),
            approval: None,
        },
        runtime::subscriber_mailbox(),
    );
    registry.register(REGISTRY_ACTOR)?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolCall};

    struct Echo;

    #[async_trait::async_trait]
    impl Tool for Echo {
        const NAME: &'static str = "echo";
        const RISK: Risk = Risk::ReadOnly;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok("hi"))
        }
    }

    #[tokio::test]
    async fn hidden_tools_are_per_session_and_inherited() {
        let reg = ToolRegistry::spawn(ToolRegistry {
            tools: HashMap::from([("echo".to_owned(), Arc::new(Echo) as DynTool)]),
            prompts: BTreeMap::new(),
            policy: Policy::default(),
            approvals: true,
            bypass: HashSet::new(),
            hidden: HashMap::new(),
            parents: HashMap::new(),
            config_path: PathBuf::new(),
            approval: None,
        });
        let (parent, child, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        let call = |s| ToolCall {
            id: "1".into(),
            name: "echo".into(),
            args: serde_json::json!({}),
            session: s,
        };
        let names = |specs: Vec<ToolSpec>| specs.into_iter().map(|s| s.name).collect::<Vec<_>>();

        reg.ask(SetHidden {
            session: parent,
            tools: vec!["echo".into()],
            hidden: true,
        })
        .await
        .unwrap();
        assert!(names(reg.ask(ListTools(Some(child))).await.unwrap()).is_empty());
        assert_eq!(
            names(reg.ask(ListTools(Some(other))).await.unwrap()),
            ["echo"]
        );
        assert_eq!(names(reg.ask(ListTools(None)).await.unwrap()), ["echo"]);
        assert!(reg.ask(Invoke(call(child))).await.unwrap().is_error);
        assert!(!reg.ask(Invoke(call(other))).await.unwrap().is_error);
        assert_eq!(
            reg.ask(HiddenTools(child)).await.unwrap(),
            HashSet::from(["echo".to_owned()])
        );

        reg.ask(SetHidden {
            session: parent,
            tools: vec!["echo".into()],
            hidden: false,
        })
        .await
        .unwrap();
        assert_eq!(
            names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo"]
        );
        assert!(!reg.ask(Invoke(call(child))).await.unwrap().is_error);
    }
}
