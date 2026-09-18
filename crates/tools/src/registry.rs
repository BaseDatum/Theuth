//! `ToolRegistry` actor, registered as [`crate::REGISTRY_ACTOR`].
//! Owns every [`DynTool`]; every invocation passes through the one approval
//! contract here before the tool runs: `Risk → PolicyAction` from
//! `tools.policy`, with `Ask` rewritten to `Allow` by the approvals switches
//! and nothing else (12 §5).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::activity::{self, Activity, ActivitySink, Kind, OperationId};
use crate::approval::{Approval, Decision, RequestApproval};
use crate::ask::{AskUser, AskUserTool};
use crate::communication::{self, Admitted, CommunicationAccess};
use crate::jobs::{InstallActivity, JOBS_ACTOR, Jobs, lock};
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
    /// The activity supervisor (47 §4), from [`RegisterActivity`]. `None`
    /// until `Agents` registers: invocations run untracked and nothing is
    /// certified. `Some` and dead: invocations fail closed.
    activity: Option<ActivitySink>,
    /// Invocations admitted while `activity` was `None`, held until their
    /// task ends. [`RegisterActivity`] adopts them, so a sink installed
    /// mid-flight starts with a blocker per running call instead of an empty
    /// ledger it would wrongly certify. Entries are inserted in the mailbox,
    /// so a registration queued behind an [`Invoke`] always sees it.
    untracked: Arc<Mutex<HashMap<OperationId, Activity>>>,
    /// Availability source for the communication tools (50 §2.2), from
    /// [`InstallCommunicationAccess`]. `None`: those tools are disabled
    /// fail-closed for every session. Not a permission input: `policy`
    /// still runs for every call this lets through.
    communication: Option<Arc<dyn CommunicationAccess>>,
    /// The caller's execution bound (50 §8), from [`InstallExecutionGate`];
    /// `None`: waits are not reported to anyone.
    gate: Option<Arc<dyn ExecutionGate>>,
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

/// `ask` → `Result<(), String>` (kameo lifts `Err` to
/// `SendError::HandlerError`). Install the activity supervisor's recipients
/// (47 §4.1). From now on every [`Invoke`] is admitted before it runs and
/// settled when its real work stops. Every invocation already running
/// untracked is reported to the sink as `Settle::Unknown` before the reply.
/// `Ok` additionally means the `Jobs` actor installed the same sink
/// ([`InstallActivity`]): nothing this crate runs is invisible to the
/// supervisor. `Err` (no jobs actor, or it did not install): invocations are
/// covered, jobs are not; the supervisor must not certify. Registering again
/// replaces the sink for new work.
pub struct RegisterActivity(pub ActivitySink);

/// `ask` → `Option<ActivitySink>`: the registered supervisor, for work
/// admitted outside any invocation (RPC job start, 47 §4.2 rule 3). Inside
/// an invocation use [`activity::current`] instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct ActivityRecipient;

/// `ask` → `()`. Install the agent-owned [`CommunicationAccess`] the
/// registry consults for `discover_peers`, `send_message` and
/// `spawn_subagent` ([`communication::GATED`]): absent from
/// [`ListTools`]`(Some(session))` and refused by [`Invoke`] with a
/// [`communication::DISABLED`] error unless it says the session is enabled,
/// both when the call arrives and again after any approval resolves. Until
/// installed those names are disabled for everyone. Installing again
/// replaces the source for new calls; calls already admitted keep the token
/// they were admitted under.
pub struct InstallCommunicationAccess(pub Arc<dyn CommunicationAccess>);

/// The caller's execution bound (50 §8), consulted by [`Invoke`]: once when
/// an invocation starts executing and around the points where it stops to
/// wait on a human — the approval ask, and the body of a tool that *is*
/// such a wait (`ask_user`). Not a permission input: it never decides
/// whether the tool runs, only whether the caller counts it as running.
pub trait ExecutionGate: Send + Sync {
    /// An invocation of `session` starts: `tool` and its raw arguments let
    /// the caller recognise a call that waits from the start (a blocking
    /// peer send). `None` when the caller bounds nothing for this session;
    /// `Some` is held until the body stopped, however it stopped.
    fn begin(&self, session: SessionId, tool: &str, args: &serde_json::Value) -> Option<Running>;

    /// An invocation of `session` is about to wait. `None` when the caller
    /// bounds nothing for this session (an ordinary human turn); `Some` is
    /// the suspension to [`Suspension::resume`] before anything executes
    /// again. Dropping it unresumed means the invocation ended in the wait
    /// (denied, timed out, cancelled): the caller retracts the wait and
    /// acquires nothing.
    fn suspend(&self, session: SessionId) -> Option<Suspension>;
}

/// A running invocation as the caller counts it; dropping it is the end.
pub type Running = Box<dyn Send>;

/// One suspended wait of an invocation; see [`ExecutionGate`].
pub struct Suspension(Box<dyn Suspended>);

/// The caller's side of a [`Suspension`].
pub trait Suspended: Send {
    /// The wait resolved and the invocation is about to execute again:
    /// complete once it may. `false` when the caller's turn is over and the
    /// invocation should not run.
    fn resume(self: Box<Self>) -> Pin<Box<dyn Future<Output = bool> + Send>>;
}

impl Suspension {
    /// Wrap a caller's [`Suspended`].
    pub fn new(inner: impl Suspended + 'static) -> Self {
        Self(Box::new(inner))
    }

    /// See [`Suspended::resume`].
    pub async fn resume(self) -> bool {
        self.0.resume().await
    }
}

/// `ask` → `()`. Install the caller's [`ExecutionGate`]; installing again
/// replaces it for new invocations.
pub struct InstallExecutionGate(pub Arc<dyn ExecutionGate>);

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

    /// Communication admission for `name` (50 §2.2): `Ok(None)` for a tool
    /// the seam does not govern, `Ok(Some(_))` for a gated tool the installed
    /// source enables for `session`, `Err` otherwise (including "nothing
    /// installed"). Consulted, never computed, here: enablement, ancestry
    /// and scope are `agent`'s.
    fn admit(
        &self,
        session: SessionId,
        name: &str,
    ) -> Result<Option<Admitted>, communication::Disabled> {
        if !communication::gated(name) {
            return Ok(None);
        }
        Admitted::check(self.communication.as_ref(), session).map(Some)
    }

    /// `name` is visible to `session` as far as communication goes: ungated,
    /// or gated and currently enabled.
    fn communicable(&self, session: SessionId, name: &str) -> bool {
        self.admit(session, name).is_ok()
    }

    /// The one permission decision (12 §5), shared by [`Invoke`] and
    /// [`ExternalApproval`]: `Ok(None)` run now, `Ok(Some(approval))` ask the
    /// human first, `Err(why)` refuse. `mentions` = the args carry a secret
    /// placeholder (37 §0.4): at least `Ask`, before the approval switches,
    /// so `/approvals off` still silences it and `Deny` is never rewritten.
    fn gate(
        &mut self,
        session: SessionId,
        risk: Risk,
        mentions: bool,
    ) -> Result<Option<ActorRef<Approval>>, &'static str> {
        let mut action = self.policy.action(risk);
        if mentions {
            action = action.max(PolicyAction::Ask);
        }
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
        let approval = match self.gate(m.session, m.risk, false) {
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
            .filter(|(name, _)| {
                session.is_none_or(|s| !self.is_hidden(s, name) && self.communicable(s, name))
            })
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

impl Message<InstallCommunicationAccess> for ToolRegistry {
    type Reply = ();

    async fn handle(
        &mut self,
        InstallCommunicationAccess(access): InstallCommunicationAccess,
        _: &mut Context<Self, Self::Reply>,
    ) {
        tracing::info!("installed communication access");
        self.communication = Some(access);
    }
}

impl Message<InstallExecutionGate> for ToolRegistry {
    type Reply = ();

    async fn handle(
        &mut self,
        InstallExecutionGate(gate): InstallExecutionGate,
        _: &mut Context<Self, Self::Reply>,
    ) {
        tracing::info!("installed execution gate");
        self.gate = Some(gate);
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

impl Message<RegisterActivity> for ToolRegistry {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RegisterActivity,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        {
            let running = lock(&self.untracked);
            for a in running.values() {
                a.adopt(&m.0);
            }
            tracing::info!(adopted = running.len(), "registered activity supervisor");
        }
        self.activity = Some(m.0.clone());
        // Jobs never asks back into this actor, so awaiting it here cannot deadlock.
        let Ok(Some(jobs)) = ActorRef::<Jobs>::lookup(JOBS_ACTOR) else {
            return Err("jobs actor not running: job coverage uncertified".into());
        };
        jobs.ask(InstallActivity(m.0))
            .await
            .map(drop)
            .map_err(|e| format!("jobs actor did not install the activity sink: {e}"))
    }
}

impl Message<ActivityRecipient> for ToolRegistry {
    type Reply = Option<ActivitySink>;

    async fn handle(
        &mut self,
        _: ActivityRecipient,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.activity.clone()
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
        // Availability before permission (50 §0.5): a gated tool that is off
        // does not exist for this session, so no approval card is ever shown.
        let admitted = match self.admit(call.session, &call.name) {
            Ok(a) => a,
            Err(why) => {
                tx.send(communication::refused(&call.name, why));
                return reply;
            }
        };
        let mentions = crate::secrets::mentions(&call.args);
        let approval = match self.gate(call.session, spec.risk, mentions) {
            Ok(a) => a,
            Err(why) => {
                tx.send(ToolOutput::err(format!("{}: {why}", call.name)));
                return reply;
            }
        };
        // Everything that awaits leaves the mailbox: admission (so `Agents`
        // can still ask this actor while deciding), approval, the tool body.
        // The `Activity` lives in this task until the output is handed back:
        // a caller dropping the ask or synthesizing a cancelled response does
        // not retire the entry; only this task finishing does (47 §4.2 rule 2).
        let sink = self.activity.clone();
        let gate = self.gate.clone();
        let kind = Kind::Invoke {
            tool: call.name.clone(),
            call_id: call.id.clone(),
            quiet: call.quiet,
        };
        // No supervisor yet: admit untracked *here*, inside the mailbox, so a
        // `RegisterActivity` behind this message adopts the call.
        let held = sink.is_none().then(|| {
            let activity = Activity::untracked(call.session, kind.clone());
            lock(&self.untracked).insert(activity.id(), activity.clone());
            Held {
                map: self.untracked.clone(),
                activity,
            }
        });
        tokio::spawn(async move {
            let activity = match &held {
                Some(h) => h.activity.clone(),
                None => match activity::admit(sink.as_ref(), call.session, kind).await {
                    Ok(a) => a,
                    Err(why) => {
                        tx.send(ToolOutput::err(format!("{}: {why}", call.name)));
                        return;
                    }
                },
            };
            // Counted as running from here until the body stopped (50 §8);
            // the caller's bound sees the end before the output does.
            let running = gate
                .as_ref()
                .and_then(|g| g.begin(call.session, &call.name, &call.args));
            let out = activity
                .clone()
                .scope(invoke(tool, spec, call, approval, admitted, activity, gate))
                .await;
            drop(running);
            tx.send(out);
            // The map entry goes when this task ends; the last clone settles.
            drop(held);
        });
        reply
    }
}

/// One pre-registration invocation's slot in `ToolRegistry::untracked`;
/// released when the invocation task ends, however it ends.
struct Held {
    map: Arc<Mutex<HashMap<OperationId, Activity>>>,
    activity: Activity,
}

impl Drop for Held {
    fn drop(&mut self) {
        lock(&self.map).remove(&self.activity.id());
    }
}

/// Approval, secret expansion and the tool body, all under `activity`'s scope
/// so nested work can read [`activity::current`]. Returns only once the tool
/// body has actually stopped (returned, failed or panicked). `admitted` is
/// the communication admission for a gated tool: re-checked once approval
/// has resolved, before anything is expanded or dispatched (50 §8). `gate`
/// is the caller's execution bound: an approval wait, and the body of a tool
/// that is itself a human wait (`ask_user`), run suspended, and nothing of
/// the body runs before the suspension resumed.
async fn invoke(
    tool: DynTool,
    spec: ToolSpec,
    mut call: ToolCall,
    approval: Option<ActorRef<Approval>>,
    admitted: Option<Admitted>,
    activity: Activity,
    gate: Option<Arc<dyn ExecutionGate>>,
) -> ToolOutput {
    let session = call.session;
    let suspend = || gate.as_ref().and_then(|g| g.suspend(session));
    if let Some(approval) = approval {
        let req = RequestApproval {
            session: call.session,
            tool: spec.name.clone(),
            args: call.args.clone(),
            risk: spec.risk,
        };
        let suspended = suspend();
        let answer = approval.ask(req).await;
        match answer {
            Ok(r) if r.decision == Decision::Allow => {}
            // Denied / timed out / unavailable: the suspension drops
            // unresumed and nothing runs.
            Ok(r) if r.timed_out => {
                return ToolOutput::err(format!("{}: approval timed out", spec.name));
            }
            Ok(_) => return ToolOutput::err(format!("{}: denied by user", spec.name)),
            Err(e) => return ToolOutput::err(format!("{}: approval unavailable: {e}", spec.name)),
        }
        if let Some(s) = suspended
            && !s.resume().await
        {
            return ToolOutput::err(format!("{}: cancelled before it ran", spec.name));
        }
    }
    // The approval may have taken minutes; the route it was asked for may be
    // gone. A revoked generation stays revoked even if the switch is on again.
    if admitted.as_ref().is_some_and(|a| !a.live()) {
        return communication::refused(&spec.name, communication::Disabled::Revoked);
    }
    // Placeholders become values after the gate (the approval card showed the
    // placeholder) and before the tool; an unknown name fails the whole call.
    if let Err(e) = crate::secrets::expand(&mut call.args) {
        return ToolOutput::err(e);
    }
    let cx = ToolCx {
        session: call.session,
        call_id: call.id,
        quiet: call.quiet,
    };
    // A tool whose whole body is a human wait executes nothing bounded; the
    // suspension ends with the body (the next call in the batch, or the
    // model, resumes under the caller's own accounting).
    let waiting = (spec.name == <AskUserTool as crate::Tool>::NAME)
        .then(suspend)
        .flatten();
    // Own task so a panicking tool is an error output, not a dead registry.
    // Task-locals do not cross `spawn`: re-enter both scopes on the spawned
    // future, so the body sees `current()` (a panic there marks the operation
    // unknown) and a gated tool reads *this* admission at dispatch rather than
    // a fresh generation minted by an off→on since the check above (50 §8).
    let body = activity.scope(communication::scope(admitted, async move {
        tool.call(call.args, cx).await
    }));
    let mut out = match tokio::spawn(body).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => ToolOutput::err(format!("{}: {e:#}", spec.name)),
        Err(e) => ToolOutput::err(format!("{}: tool panicked: {e}", spec.name)),
    };
    if let Some(w) = waiting {
        // Nothing runs after this in the invocation; resuming only puts the
        // batch's accounting back (a cancelled turn makes it a no-op).
        w.resume().await;
    }
    // Every arm: an error string may echo the expanded args.
    crate::secrets::redact_output(&mut out);
    out
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
            activity: None,
            untracked: Arc::default(),
            communication: None,
            gate: None,
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

    fn registry(tools: Vec<DynTool>, activity: Option<ActivitySink>) -> ActorRef<ToolRegistry> {
        registry_with(tools, activity, Policy::default(), None)
    }

    fn registry_with(
        tools: Vec<DynTool>,
        activity: Option<ActivitySink>,
        policy: Policy,
        approval: Option<ActorRef<Approval>>,
    ) -> ActorRef<ToolRegistry> {
        ToolRegistry::spawn(ToolRegistry {
            tools: tools.into_iter().map(|t| (t.spec().name, t)).collect(),
            prompts: BTreeMap::new(),
            policy,
            approvals: true,
            bypass: HashSet::new(),
            hidden: HashMap::new(),
            parents: HashMap::new(),
            config_path: PathBuf::new(),
            approval,
            activity,
            untracked: Arc::default(),
            communication: None,
            gate: None,
        })
    }

    fn call(name: &str, s: SessionId) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            args: serde_json::json!({}),
            session: s,
            quiet: false,
        }
    }

    /// Blocks until released; reports the `current()` op it ran under.
    struct Slow {
        release: Arc<tokio::sync::Notify>,
        ran_under: Arc<std::sync::Mutex<Option<Option<activity::OperationId>>>>,
    }

    #[async_trait::async_trait]
    impl Tool for Slow {
        const NAME: &'static str = "slow";
        const RISK: Risk = Risk::ReadOnly;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            *jobs::lock(&self.ran_under) = Some(activity::current().map(|a| a.id()));
            self.release.notified().await;
            Ok(ToolOutput::ok("done"))
        }
    }

    #[tokio::test]
    async fn invocation_is_admitted_first_and_outlives_a_dropped_ask() {
        use crate::activity::test_sink::{Seen, drained, spawn};
        let (ledger, sink, seen) = spawn(None);
        let release = Arc::new(tokio::sync::Notify::new());
        let ran_under = Arc::new(std::sync::Mutex::new(None));
        let slow = Slow {
            release: release.clone(),
            ran_under: ran_under.clone(),
        };
        let reg = registry(vec![Arc::new(slow)], Some(sink));
        let session = SessionId::new();

        // Caller gives up: synthetic cancellation upstream, real work continues.
        let ask = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ask(Invoke(call("slow", session))).await }
        });
        while jobs::lock(&ran_under).is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        ask.abort();
        let _ = ask.await;

        let seen_now = drained(&ledger, &seen).await;
        let [Seen::Admitted(op)] = seen_now.as_slice() else {
            panic!("expected one admission, got {seen_now:?}");
        };
        assert_eq!(
            (op.owner, &op.kind),
            (
                session,
                &Kind::Invoke {
                    tool: "slow".into(),
                    call_id: "1".into(),
                    quiet: false
                }
            )
        );
        assert_eq!(
            *jobs::lock(&ran_under),
            Some(Some(op.id)),
            "body sees its own activity"
        );

        release.notify_one();
        let settled = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let s = drained(&ledger, &seen).await;
                if s.len() > 1 {
                    return s;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("settles once the body returns");
        assert_eq!(settled[1], Seen::Finished(op.id));
    }

    #[tokio::test]
    async fn rejected_or_lost_supervisor_fails_closed_without_running() {
        use crate::activity::test_sink::spawn;
        let ran_under = Arc::new(std::sync::Mutex::new(None));
        let tool = || -> DynTool {
            Arc::new(Slow {
                release: Arc::new(tokio::sync::Notify::new()),
                ran_under: ran_under.clone(),
            })
        };
        let session = SessionId::new();

        let (_ledger, sink, _) = spawn(Some("paused"));
        let out = registry(vec![tool()], Some(sink))
            .ask(Invoke(call("slow", session)))
            .await
            .unwrap();
        assert!(out.is_error && out.content.contains("paused"), "{out:?}");

        let (ledger, sink, _) = spawn(None);
        ledger.stop_gracefully().await.unwrap();
        ledger.wait_for_shutdown().await;
        let out = registry(vec![tool()], Some(sink))
            .ask(Invoke(call("slow", session)))
            .await
            .unwrap();
        assert!(
            out.is_error && out.content.contains("unavailable"),
            "{out:?}"
        );
        assert!(jobs::lock(&ran_under).is_none(), "tool body never ran");
    }

    #[tokio::test]
    async fn registration_adopts_a_running_untracked_invocation() {
        use crate::activity::test_sink::{Seen, drained, spawn};
        let release = Arc::new(tokio::sync::Notify::new());
        let ran_under = Arc::new(std::sync::Mutex::new(None));
        let slow = Slow {
            release: release.clone(),
            ran_under: ran_under.clone(),
        };
        let reg = registry(vec![Arc::new(slow)], None);
        let session = SessionId::new();

        // Started with no supervisor: runs untracked.
        let ask = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ask(Invoke(call("slow", session))).await }
        });
        while jobs::lock(&ran_under).is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let op = jobs::lock(&ran_under)
            .flatten()
            .expect("body runs under a guard");

        // Registration lands mid-flight: the ack must not certify an empty ledger.
        let (ledger, sink, seen) = spawn(None);
        // No jobs actor here: invocations are covered, the reply says jobs are not.
        assert!(matches!(
            reg.ask(RegisterActivity(sink)).await,
            Err(kameo::error::SendError::HandlerError(_))
        ));
        let now = drained(&ledger, &seen).await;
        assert!(
            matches!(now.as_slice(), [Seen::Unknown(o, why)] if *o == op && why.contains("before")),
            "{now:?}"
        );

        // Settles through the adopted sink once the body returns.
        release.notify_one();
        assert!(!ask.await.unwrap().unwrap().is_error);
        let settled = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let s = drained(&ledger, &seen).await;
                if s.len() > 1 {
                    return s;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("adopted call settles");
        assert_eq!(settled[1], Seen::Finished(op));
    }

    #[tokio::test]
    async fn hidden_tools_are_per_session_and_inherited() {
        let reg = registry(vec![Arc::new(Echo)], None);
        let (parent, child, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        let call = |s| call("echo", s);
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

    /// Stands in for the agent's `send_message`: counts body runs.
    struct Peer(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl Tool for Peer {
        const NAME: &'static str = "send_message";
        const RISK: Risk = Risk::Mutating;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolOutput::ok("sent"))
        }
    }

    /// Fake agent-side source: one switch, a fresh generation token per enable,
    /// the old one cancelled on disable before the switch reads off.
    struct Switch {
        on: std::sync::atomic::AtomicBool,
        token: Mutex<tokio_util::sync::CancellationToken>,
    }

    impl Switch {
        fn new(on: bool) -> Arc<Self> {
            Arc::new(Self {
                on: on.into(),
                token: Mutex::new(tokio_util::sync::CancellationToken::new()),
            })
        }
        fn set(&self, on: bool) {
            if on {
                *lock(&self.token) = tokio_util::sync::CancellationToken::new();
            } else {
                lock(&self.token).cancel();
            }
            self.on.store(on, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl CommunicationAccess for Switch {
        fn enabled(&self, _: SessionId) -> bool {
            self.on.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn generation(&self, _: SessionId) -> Option<tokio_util::sync::CancellationToken> {
            Some(lock(&self.token).clone())
        }
    }

    #[tokio::test]
    async fn communication_tools_are_disabled_until_installed_and_when_off() {
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let allow = Policy {
            mutating: PolicyAction::Allow,
            ..Policy::default()
        };
        let reg = registry_with(
            vec![Arc::new(Peer(ran.clone())), Arc::new(Echo)],
            None,
            allow,
            None,
        );
        let s = SessionId::new();
        let names = |specs: Vec<ToolSpec>| specs.into_iter().map(|s| s.name).collect::<Vec<_>>();
        let send = || reg.ask(Invoke(call("send_message", s)));

        // Nothing installed: gated names do not exist for a session, other tools do.
        assert_eq!(names(reg.ask(ListTools(Some(s))).await.unwrap()), ["echo"]);
        // Session-less inventory (RPC list, MCP name-clash check) is unfiltered.
        assert_eq!(
            names(reg.ask(ListTools(None)).await.unwrap()),
            ["echo", "send_message"]
        );
        let out = send().await.unwrap();
        assert!(
            out.is_error && out.content.contains(communication::DISABLED),
            "{out:?}"
        );
        assert!(out.content.contains("not configured"), "{out:?}");
        assert!(!reg.ask(Invoke(call("echo", s))).await.unwrap().is_error);

        let sw = Switch::new(false);
        reg.ask(InstallCommunicationAccess(sw.clone()))
            .await
            .unwrap();
        assert_eq!(names(reg.ask(ListTools(Some(s))).await.unwrap()), ["echo"]);
        let out = send().await.unwrap();
        assert!(
            out.is_error && out.content.contains(communication::DISABLED),
            "{out:?}"
        );
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 0);

        sw.set(true);
        assert_eq!(
            names(reg.ask(ListTools(Some(s))).await.unwrap()),
            ["echo", "send_message"]
        );
        assert!(!send().await.unwrap().is_error);
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);

        sw.set(false);
        assert!(send().await.unwrap().is_error);
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Bus subscriber standing in for the human: on the first request flips
    /// the switch as instructed, then allows; later requests are plain allows.
    #[derive(kameo::Actor)]
    struct Approver {
        approval: ActorRef<Approval>,
        flip: Option<(Arc<Switch>, bool)>,
    }

    impl Message<crate::approval::ApprovalRequested> for Approver {
        type Reply = ();
        async fn handle(
            &mut self,
            m: crate::approval::ApprovalRequested,
            _: &mut Context<Self, ()>,
        ) {
            if let Some((sw, reenable)) = self.flip.take() {
                sw.set(false);
                if reenable {
                    sw.set(true);
                }
            }
            let _ = self
                .approval
                .ask(crate::approval::Resolve {
                    id: m.id,
                    decision: Decision::Allow,
                })
                .await;
        }
    }

    #[tokio::test]
    async fn approved_call_is_refused_when_its_generation_was_revoked_meanwhile() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let approval = Approval::spawn(Approval::new(rt.clone(), Duration::from_secs(5)));
        let sw = Switch::new(true);
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reg = registry_with(
            vec![Arc::new(Peer(ran.clone()))],
            None,
            Policy::default(),
            Some(approval.clone()),
        );
        reg.ask(InstallCommunicationAccess(sw.clone()))
            .await
            .unwrap();
        let s = SessionId::new();

        // Off then on again while the human deliberates: the switch reads on
        // when the approval lands, but the admitted generation is gone.
        let approver = Approver::spawn(Approver {
            approval,
            flip: Some((sw.clone(), true)),
        });
        rt.subscribe::<Approver, crate::approval::ApprovalRequested>(&approver)
            .await;
        let out = reg.ask(Invoke(call("send_message", s))).await.unwrap();
        assert!(
            out.is_error && out.content.contains(communication::DISABLED),
            "{out:?}"
        );
        assert!(out.content.contains("while this call waited"), "{out:?}");
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(sw.enabled(s), "the switch itself is back on");

        // A fresh call is admitted under the new generation and runs.
        assert!(
            !reg.ask(Invoke(call("send_message", s)))
                .await
                .unwrap()
                .is_error
        );
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
