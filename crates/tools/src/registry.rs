//! `ToolRegistry` actor, registered as [`crate::REGISTRY_ACTOR`].
//! Owns every [`DynTool`]; every invocation passes through the one approval
//! contract here before the tool runs: `Risk → PolicyAction` from
//! `tools.policy`, with `Ask` rewritten to `Allow` by the approvals switches
//! and nothing else (12 §5) — or, on the app entry alone, answered `Allow`
//! by the user's own activation of that action (46 §6.1).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brush_core::Shell;
use kameo::actor::{ActorRef, Recipient, ReplyRecipient, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::activity::{self, Activity, ActivitySink, Kind, OperationId};
use crate::approval::{Approval, Decision, RequestApproval};
use crate::ask::{AskUser, AskUserTool};
use crate::communication::{self, Admitted, CommunicationAccess};
use crate::jobs::{CloseSessions, InstallActivity, JOBS_ACTOR, Jobs, lock};
use crate::resources::{CloseAck, CloseTree, SessionResources, TreeOpened};
use crate::{
    APPROVAL_ACTOR, ASK_ACTOR, DynTool, Policy, PolicyAction, REGISTRY_ACTOR, Risk, ToolCall,
    ToolCx, ToolOutput, ToolSource, ToolSpec, ToolsConfig, edit, fs, glob, jobs, pty, schema,
    shell,
};

/// [`CloseTreeAll`]'s wait on one participant before its ack is `timeout`.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(30);

/// The registry actor. Look up with `ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)`.
/// `policy` is the only permission input; a tool's `ToolSpec.risk` is the
/// only key into it (12 §5).
#[derive(kameo::Actor)]
pub struct ToolRegistry {
    tools: HashMap<String, DynTool>,
    /// Compiled argument schema per registered tool, built on the first
    /// [`AppInvoke`] for that tool and dropped with the registration (46 §7:
    /// polling apps must not pay a compile per call). Keyed like `tools`, so
    /// it can never hold more than the tools that exist.
    validators: HashMap<String, schema::Compiled>,
    /// Plugin system-prompt blocks by plugin name (07 §4 part 7): sorted, so
    /// the rendered prompt is stable across restarts.
    prompts: BTreeMap<&'static str, String>,
    /// Fence checkers by language from [`RegisterFenceChecker`].
    fence_checkers: HashMap<String, Arc<dyn FenceChecker>>,
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
    /// Risk ceiling per session from [`SetCap`]: a tool above it is neither
    /// listed for nor runnable by the session or its subagents, except
    /// [`CAP_EXEMPT`]. In memory only, like `hidden`.
    cap: HashMap<SessionId, Risk>,
    /// `<data_dir>/config.yaml`, for the persistent switch.
    config_path: PathBuf,
    /// Per-root file capability from [`InstallResources`] (51 §4.5); a
    /// session without one resolves through `parents`, then to `durable`.
    resources: HashMap<SessionId, Arc<SessionResources>>,
    /// [`SessionResources::durable`] of the data dir: what every session gets
    /// unless a private root was installed.
    durable: Arc<SessionResources>,
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
    /// Lifecycle owners from [`RegisterLifecycle`] (51 §4.4), by name; a
    /// second registration under a name replaces the first.
    participants: Vec<(
        &'static str,
        Option<Recipient<TreeOpened>>,
        ReplyRecipient<CloseTree, CloseAck>,
    )>,
    /// Owner names [`ExpectParticipants`] said must register before
    /// [`ParticipantsReady`] says so.
    expected: Vec<&'static str>,
    /// The `shell` tool's per-session shells, dropped for a closed subtree.
    shells: Arc<Mutex<HashMap<SessionId, Shell>>>,
    /// How long [`CloseTreeAll`] waits on one participant.
    close_timeout: Duration,
    /// `tools.pty_idle_secs`, shared with the `start` tool (61 §1.6); the
    /// persistent knob is [`SetPtyIdle`].
    pty_idle: Arc<AtomicU64>,
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

/// `ask` → [`AppInvoked`]. The app bridge's entry (46 §6): the same tool
/// map, visibility, policy, activity admission, secret expansion and body as
/// [`Invoke`], plus what only a published app has: a bound identity to hold
/// the tool to, an argument re-check against the current schema, and
/// consent evidence. Never fails. Not for model calls: those go through
/// [`Invoke`] and keep asking.
#[derive(Debug)]
pub struct AppInvoke {
    /// `call.id` is the app request id (stable across transport retries).
    pub call: ToolCall,
    /// The press that caused this call, or `None` for an automatic query
    /// (load, refresh, state change).
    pub activation: Option<UserActivation>,
    /// [`ToolInspection::identity`] recorded when the app was published; any
    /// other current identity is [`AppInvoked::Unavailable`], never a rebind.
    pub expected_identity: String,
}

/// Evidence that a human activated one action of one committed app record,
/// constructed by the app bridge after validating the binding. It answers
/// `Ask` for exactly the call whose id equals `request_id`: it is not
/// inherited by later refreshes, other `@Run` targets or the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserActivation {
    /// Artifact record (`call_id`) of the app version that was on screen.
    pub record_call_id: String,
    /// The action execution the renderer created on real activation.
    pub action_id: String,
    /// The one invocation this covers; must equal [`ToolCall::id`].
    pub request_id: String,
}

/// Outcome of [`AppInvoke`].
#[derive(Clone, Debug, kameo::Reply)]
pub enum AppInvoked {
    /// Dispatched; ran to an output (which may itself be an error).
    Completed(ToolOutput),
    /// Effective policy is `Ask` and nothing activated it: nothing ran, no
    /// prompt was shown. The app renders a Load/Refresh affordance whose
    /// press is the approval (46 §6.1).
    NeedsApproval,
    /// Never dispatched: unknown or hidden tool, identity changed since
    /// publication, arguments fail the current schema, activation does not
    /// cover this call, or policy is `Deny` (final, pressed or not).
    Unavailable(String),
}

/// Why [`ToolRegistry::admit_app`] refused a call: [`AppInvoked`] minus
/// `Completed`, so the `Err` arm stays a `String` wide.
enum Refused {
    NeedsApproval,
    Unavailable(String),
}

impl From<Refused> for AppInvoked {
    fn from(r: Refused) -> Self {
        match r {
            Refused::NeedsApproval => Self::NeedsApproval,
            Refused::Unavailable(why) => Self::Unavailable(why),
        }
    }
}

/// `ask` → `Result<ToolInspection, String>`: one visible tool's spec,
/// origin, identity and what the policy would do for `session` right now.
/// `args`, when known, raises `action` for secret placeholders like the
/// gate does; `None` reports the policy of the tool alone (publish time).
/// `Err` = unknown or hidden.
#[derive(Clone, Debug)]
pub struct InspectTool {
    /// Session whose visibility and approval switches apply.
    pub session: SessionId,
    /// Registered tool name.
    pub name: String,
    /// Arguments about to be sent, if known.
    pub args: Option<serde_json::Value>,
}

/// `ask` → `Vec<ToolInspection>`, sorted by name: [`ListTools`] with origin
/// and identity, for the publish-time snapshot a parser check binds to.
/// `action` is the tool's own policy (no args).
#[derive(Clone, Copy, Debug, Default)]
pub struct DescribeTools(pub Option<SessionId>);

/// What an app binds to and shows for one tool.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ToolInspection {
    /// The spec as the model sees it.
    pub spec: ToolSpec,
    /// Native, or the MCP server entry without credentials. Display only.
    pub source: ToolSource,
    /// The binding: sha256 hex over the name, schema and the tool's private
    /// origin fingerprint ([`crate::ErasedTool::origin`]). Opaque: store and
    /// compare it, never recompute it. Risk/policy are not part of it.
    pub identity: String,
    /// Effective action for the asking session (`Ask` = an automatic app
    /// call comes back [`AppInvoked::NeedsApproval`]).
    pub action: PolicyAction,
}

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

/// `ask` → [`Decision`]: the same gate as [`Invoke`] for a tool call theuth does
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

/// `ask` → every registered `(plugin, block)`, sorted by plugin name.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListPrompts;

/// One located error from a [`FenceChecker`]: `path` is the checker's
/// locator (`root`, `$stage`, `line:4`, …) or `""` when it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FenceError {
    /// Where: the checker's locator, or `""` for the whole block.
    pub path: String,
    /// What, in the parser's words; shown to the model verbatim.
    pub message: String,
}

/// Validates the body of one reply fence (30 §5): a plugin registers one per
/// language it can parse (`mermaid`, `openui`). `Err` = the checker is
/// unavailable (helper missing, crashed, timed out); the loop treats such a
/// fence as valid rather than blaming the model.
#[async_trait::async_trait]
pub trait FenceChecker: Send + Sync {
    /// Located errors for `body`; empty means it renders.
    async fn check(&self, body: &str) -> Result<Vec<FenceError>, String>;
}

/// `ask` → `()`. Installs the checker for one fence language; replaces a
/// previous one for the same language.
pub struct RegisterFenceChecker {
    /// Fence info string (`mermaid`, `openui`).
    pub lang: String,
    /// The checker.
    pub checker: Arc<dyn FenceChecker>,
}

/// `ask` → `None` when no checker is registered for `lang`, else the
/// checker's result.
pub struct CheckFence {
    /// Fence info string.
    pub lang: String,
    /// Fence body without the fence lines.
    pub body: String,
}

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

/// `ask` → `()`: persist `tools.pty_idle_secs` (61 §1.6) and use it for
/// every later `start` without `idle_secs`; `0` = never.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetPtyIdle {
    /// Quiet-screen seconds before `<job_idle>`.
    pub seconds: u64,
}

/// `ask` → `u64`: the live `tools.pty_idle_secs`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetPtyIdle;

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

/// `tell`: cap `session` (and its subagents) at `cap` — tools above it are
/// not offered and a call to one is `denied: subagent is capped`. `None`
/// clears the session's own entry; an ancestor's still applies.
#[derive(Clone, Copy, Debug)]
pub struct SetCap {
    /// Session to cap.
    pub session: SessionId,
    /// The ceiling; `None` removes it.
    pub cap: Option<Risk>,
}

impl Message<SetCap> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetCap, _: &mut Context<Self, Self::Reply>) {
        match m.cap {
            Some(cap) => self.cap.insert(m.session, cap),
            None => self.cap.remove(&m.session),
        };
    }
}

/// Tools a capped subagent keeps whatever its ceiling: reporting to its
/// parent is what a subagent is for, and `send_message` is `Mutating` for
/// the approval policy (a peer's inbox changes), not because it touches
/// anything the cap protects. The policy still runs for these.
const CAP_EXEMPT: &[&str] = &["send_message"];

/// `tell`: `session`'s owners write under `resources` from now on (51 §4.5).
/// The supervisor installs one per private root before its first turn;
/// subagents inherit through [`SetParent`].
#[derive(Clone, Debug)]
pub struct InstallResources {
    /// The root whose tool writes are being redirected.
    pub session: SessionId,
    /// Where `session` and its subagents write from now on.
    pub resources: Arc<SessionResources>,
}

impl Message<InstallResources> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: InstallResources, _: &mut Context<Self, Self::Reply>) {
        self.resources.insert(m.session, m.resources);
    }
}

/// `tell`: forget the root's [`InstallResources`] on close; later calls for
/// the id fall back to durable (and should not happen).
#[derive(Clone, Copy, Debug)]
pub struct RemoveResources(pub SessionId);

impl Message<RemoveResources> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: RemoveResources, _: &mut Context<Self, Self::Reply>) {
        self.resources.remove(&m.0);
    }
}

/// `ask` → `()`. An owner joins the lifecycle barrier (51 §4.4): `opened`,
/// when given, is told every [`OpenTree`]; `close` is asked every
/// [`CloseTreeAll`]. Registering the same `owner` again replaces the first.
pub struct RegisterLifecycle {
    /// The owner's name, as [`ExpectParticipants`] lists it and acks carry it.
    pub owner: &'static str,
    /// Told each [`TreeOpened`]; `None` for an owner with nothing to prepare.
    pub opened: Option<Recipient<TreeOpened>>,
    /// Asked each [`CloseTree`].
    pub close: ReplyRecipient<CloseTree, CloseAck>,
}

/// `ask` → `()`. The names of the enabled owners that must have registered
/// before [`ParticipantsReady`] says so; sent once by composition. `"jobs"`
/// is internal and never listed.
#[derive(Clone, Debug)]
pub struct ExpectParticipants(pub Vec<&'static str>);

/// `ask` → `Result<(), String>`: `Ok` once every [`ExpectParticipants`] name
/// registered; `Err` names the missing ones, comma-joined.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParticipantsReady;

/// `ask` → `()`. Installs `resources` for every listed session and tells
/// every participant's `opened` recipient (off the mailbox, fire-and-forget).
#[derive(Clone, Debug)]
pub struct OpenTree(pub TreeOpened);

/// `ask` → `Vec<CloseAck>`: every participant's answer to the [`CloseTree`]
/// (a participant past the per-owner timeout or unreachable answers
/// `ok: false`), plus Jobs' own once it cancelled and reaped the subtree's
/// jobs and removed their logs. The subtree's shells and this actor's
/// `resources`/parent/hidden/bypass/cap entries go with it.
#[derive(Clone, Debug)]
pub struct CloseTreeAll(pub CloseTree);

/// `tell` from the [`CloseTreeAll`] task: the subtree's entries go.
struct Forget(Vec<SessionId>);

impl Message<RegisterLifecycle> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: RegisterLifecycle, _: &mut Context<Self, Self::Reply>) {
        self.participants.retain(|(o, ..)| *o != m.owner);
        self.participants.push((m.owner, m.opened, m.close));
        tracing::info!(owner = m.owner, "registered lifecycle participant");
    }
}

impl Message<ExpectParticipants> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: ExpectParticipants, _: &mut Context<Self, Self::Reply>) {
        self.expected = m.0;
    }
}

impl Message<ParticipantsReady> for ToolRegistry {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        _: ParticipantsReady,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let missing: Vec<&str> = self
            .expected
            .iter()
            .copied()
            .filter(|e| !self.participants.iter().any(|(o, ..)| o == e))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing.join(", "))
        }
    }
}

impl Message<OpenTree> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: OpenTree, _: &mut Context<Self, Self::Reply>) {
        for s in &m.0.sessions {
            self.resources.insert(*s, Arc::clone(&m.0.resources));
        }
        let opened: Vec<_> = self
            .participants
            .iter()
            .filter_map(|(_, o, _)| o.clone())
            .collect();
        tokio::spawn(async move {
            for o in opened {
                let _ = o.tell(m.0.clone()).await;
            }
        });
    }
}

impl Message<CloseTreeAll> for ToolRegistry {
    type Reply = DelegatedReply<Vec<CloseAck>>;

    async fn handle(
        &mut self,
        m: CloseTreeAll,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let tree = m.0;
        let parts: Vec<_> = self
            .participants
            .iter()
            .map(|(o, _, c)| (*o, c.clone()))
            .collect();
        let jobs = ActorRef::<Jobs>::lookup(JOBS_ACTOR).ok().flatten();
        let timeout = self.close_timeout;
        let shells = Arc::clone(&self.shells);
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let owners = parts.into_iter().map(|(owner, close)| {
                let tree = tree.clone();
                async move {
                    match tokio::time::timeout(timeout, close.ask(tree)).await {
                        Ok(Ok(ack)) => ack,
                        Ok(Err(e)) => CloseAck::failed(owner, e.to_string()),
                        Err(_) => CloseAck::failed(owner, "timeout"),
                    }
                }
            });
            let jobs = async {
                let Some(jobs) = jobs else {
                    return CloseAck::failed("jobs", "jobs actor not running");
                };
                jobs.ask(CloseSessions {
                    sessions: tree.subtree.clone(),
                    cancel: tree.cancel.clone(),
                })
                .await
                .unwrap_or_else(|e| CloseAck::failed("jobs", e.to_string()))
            };
            let (mut acks, jobs) = futures::join!(futures::future::join_all(owners), jobs);
            acks.push(jobs);
            {
                let mut shells = lock(&shells);
                for s in &tree.subtree {
                    shells.remove(s);
                }
            }
            let _ = me.tell(Forget(tree.subtree)).await;
            tx.send(acks);
        });
        reply
    }
}

impl Message<Forget> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: Forget, _: &mut Context<Self, Self::Reply>) {
        for s in &m.0 {
            self.resources.remove(s);
            self.parents.remove(s);
            self.hidden.remove(s);
            self.bypass.remove(s);
            self.cap.remove(s);
        }
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

impl Message<SetPtyIdle> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetPtyIdle, _: &mut Context<Self, Self::Reply>) {
        self.pty_idle.store(m.seconds, Ordering::SeqCst);
        if let Err(e) = runtime::write_config_value(
            &self.config_path,
            "tools",
            "pty_idle_secs",
            &m.seconds.to_string(),
        ) {
            tracing::warn!("config.yaml: {e}");
        }
    }
}

impl Message<GetPtyIdle> for ToolRegistry {
    type Reply = u64;

    async fn handle(&mut self, _: GetPtyIdle, _: &mut Context<Self, Self::Reply>) -> u64 {
        self.pty_idle.load(Ordering::SeqCst)
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

    /// The tightest [`SetCap`] on the session or an ancestor: a child never
    /// exceeds an ancestor's ceiling. `None` = no ceiling.
    fn cap_for(&self, mut id: SessionId) -> Option<Risk> {
        let mut cap: Option<Risk> = None;
        loop {
            if let Some(c) = self.cap.get(&id) {
                cap = Some(cap.map_or(*c, |k| k.min(*c)));
            }
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return cap,
            }
        }
    }

    /// The cap that puts `name` at `risk` out of the session's reach, if any:
    /// `risk` above [`Self::cap_for`] and the tool not in [`CAP_EXEMPT`].
    fn capped(&self, id: SessionId, name: &str, risk: Risk) -> Option<Risk> {
        self.cap_for(id)
            .filter(|cap| risk > *cap && !CAP_EXEMPT.contains(&name))
    }

    /// The session's [`InstallResources`], else the nearest ancestor's, else durable.
    fn resources_for(&self, mut id: SessionId) -> Arc<SessionResources> {
        loop {
            if let Some(r) = self.resources.get(&id) {
                return r.clone();
            }
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return self.durable.clone(),
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

    /// The tool under `name` as `session` sees it: `None` if unregistered or
    /// hidden for the session or an ancestor.
    fn visible(&self, session: SessionId, name: &str) -> Option<DynTool> {
        self.tools
            .get(name)
            .filter(|_| !self.is_hidden(session, name))
            .cloned()
    }

    /// The one permission decision (12 §5), before anyone is asked: `Deny`
    /// for a tool above the session's cap ([`SetCap`]), else `tools.policy`
    /// for the risk, at least `Ask` when the args carry a secret placeholder
    /// (37 §0.4), then `Ask` → `Allow` by the approval switches (global, or
    /// the session / an ancestor opted out). `Deny` is never rewritten.
    fn effective(
        &self,
        session: Option<SessionId>,
        name: &str,
        risk: Risk,
        mentions: bool,
    ) -> PolicyAction {
        if session.is_some_and(|s| self.capped(s, name, risk).is_some()) {
            return PolicyAction::Deny;
        }
        let mut action = self.policy.action(risk);
        if mentions {
            action = action.max(PolicyAction::Ask);
        }
        if action == PolicyAction::Ask
            && (!self.approvals || session.is_some_and(|s| self.bypassed(s)))
        {
            action = PolicyAction::Allow;
        }
        action
    }

    /// Why a `Deny` from [`Self::effective`] happened, for the model: the cap
    /// names its ceiling so the subagent knows the refusal is not the policy's.
    fn denied(&self, session: SessionId, name: &str, risk: Risk) -> String {
        match self.capped(session, name, risk) {
            Some(cap) => format!("denied: subagent is capped at {cap}"),
            None => "denied by policy".to_owned(),
        }
    }

    /// [`Self::effective`] for a call theuth runs or vouches for, shared by
    /// [`Invoke`] and [`ExternalApproval`]: `Ok(None)` run now,
    /// `Ok(Some(approval))` ask the human first, `Err(why)` refuse.
    fn gate(
        &mut self,
        session: SessionId,
        name: &str,
        risk: Risk,
        mentions: bool,
    ) -> Result<Option<ActorRef<Approval>>, String> {
        match self.effective(Some(session), name, risk, mentions) {
            PolicyAction::Allow => Ok(None),
            PolicyAction::Deny => Err(self.denied(session, name, risk)),
            PolicyAction::Ask => {
                if self.approval.is_none() {
                    self.approval = ActorRef::<Approval>::lookup(APPROVAL_ACTOR).ok().flatten();
                }
                self.approval
                    .clone()
                    .map(Some)
                    .ok_or_else(|| "approval required but no approval actor is running".to_owned())
            }
        }
    }

    /// Spec, origin, identity and effective action of one tool.
    fn inspect(
        &self,
        tool: &DynTool,
        session: Option<SessionId>,
        mentions: bool,
    ) -> ToolInspection {
        let spec = tool.spec();
        ToolInspection {
            action: self.effective(session, &spec.name, spec.risk, mentions),
            identity: identity(tool, &spec),
            source: tool.source(),
            spec,
        }
    }

    /// Everything an app call must pass before it is dispatched like any
    /// other (46 §6.3): visibility, communication availability (50 §0.5),
    /// the bound identity, the current schema, that the activation is for
    /// *this* call, then policy. `Err` is the reply for a call that never ran.
    fn admit_app(
        &mut self,
        call: &ToolCall,
        activation: Option<&UserActivation>,
        expected_identity: &str,
    ) -> Result<(DynTool, ToolSpec, Option<Admitted>), Refused> {
        use Refused::Unavailable;
        let name = &call.name;
        let tool = self
            .visible(call.session, name)
            .ok_or_else(|| Unavailable(format!("unknown tool: {name}")))?;
        // Availability before permission: a gated tool that is off does not
        // exist for this session, so no activation is ever asked for.
        let admitted = self
            .admit(call.session, name)
            .map_err(|why| Unavailable(format!("{name}: {}: {why}", communication::DISABLED)))?;
        let spec = tool.spec();
        if identity(&tool, &spec) != expected_identity {
            return Err(Unavailable(format!(
                "{name}: tool identity changed since the app was published; republish to rebind"
            )));
        }
        let compiled = self
            .validators
            .entry(name.clone())
            .or_insert_with(|| schema::compile(&spec.schema));
        if let Err(e) = schema::check(compiled, &call.args) {
            return Err(Unavailable(format!("{name}: invalid arguments: {e}")));
        }
        if let Some(a) = activation
            && a.request_id != call.id
        {
            return Err(Unavailable(format!(
                "{name}: activation {} covers request {}, not {}",
                a.action_id, a.request_id, call.id
            )));
        }
        let mentions = mentions(&tool, &call.args);
        match self.effective(Some(call.session), name, spec.risk, mentions) {
            PolicyAction::Allow => {}
            PolicyAction::Deny => {
                let why = self.denied(call.session, name, spec.risk);
                return Err(Unavailable(format!("{name}: {why}")));
            }
            PolicyAction::Ask => match activation {
                None => return Err(Refused::NeedsApproval),
                Some(a) => tracing::info!(
                    tool = %name, risk = ?spec.risk, secrets = mentions,
                    record = %a.record_call_id, action = %a.action_id, request = %a.request_id,
                    "app call approved by the user's activation"
                ),
            },
        }
        Ok((tool, spec, admitted))
    }

    /// Admission, approval (if `approval`), secret expansion and the tool
    /// body, off the mailbox; `done` receives the output when the body has
    /// actually stopped. Shared by [`Invoke`] and [`AppInvoke`]: the app
    /// entry only ever arrives here with `approval == None`, its `Ask`
    /// already answered by the activation in [`Self::admit_app`]. `admitted`
    /// is the communication admission from [`Self::admit`], re-checked in
    /// [`invoke`] once approval resolved (50 §8).
    fn dispatch(
        &mut self,
        tool: DynTool,
        spec: ToolSpec,
        call: ToolCall,
        approval: Option<ActorRef<Approval>>,
        admitted: Option<Admitted>,
        done: impl FnOnce(ToolOutput) + Send + 'static,
    ) {
        // Everything that awaits leaves the mailbox: admission (so `Agents`
        // can still ask this actor while deciding), approval, the tool body.
        // The `Activity` lives in this task until the output is handed back:
        // a caller dropping the ask or synthesizing a cancelled response does
        // not retire the entry; only this task finishing does (47 §4.2 rule 2).
        let sink = self.activity.clone();
        let gate = self.gate.clone();
        let resources = self.resources_for(call.session);
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
                        done(ToolOutput::err(format!("{}: {why}", call.name)));
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
                .scope(invoke(
                    tool, spec, call, approval, admitted, activity, gate, resources,
                ))
                .await;
            drop(running);
            done(out);
            // The map entry goes when this task ends; the last clone settles.
            drop(held);
        });
    }
}

impl Message<AppInvoke> for ToolRegistry {
    type Reply = DelegatedReply<AppInvoked>;

    async fn handle(&mut self, m: AppInvoke, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.admit_app(&m.call, m.activation.as_ref(), &m.expected_identity) {
            Err(r) => tx.send(r.into()),
            Ok((tool, spec, admitted)) => {
                self.dispatch(tool, spec, m.call, None, admitted, move |out| {
                    tx.send(AppInvoked::Completed(out));
                });
            }
        }
        reply
    }
}

impl Message<InspectTool> for ToolRegistry {
    type Reply = Result<ToolInspection, String>;

    async fn handle(&mut self, m: InspectTool, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let tool = self
            .visible(m.session, &m.name)
            .ok_or_else(|| format!("unknown tool: {}", m.name))?;
        let mentions = m.args.as_ref().is_some_and(|a| mentions(&tool, a));
        Ok(self.inspect(&tool, Some(m.session), mentions))
    }
}

impl Message<DescribeTools> for ToolRegistry {
    type Reply = Vec<ToolInspection>;

    async fn handle(
        &mut self,
        DescribeTools(session): DescribeTools,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut out: Vec<ToolInspection> = self
            .tools
            .iter()
            .filter(|(name, _)| !session.is_some_and(|s| self.is_hidden(s, name)))
            .map(|(_, t)| self.inspect(t, session, false))
            .filter(|i| {
                !session.is_some_and(|s| self.capped(s, &i.spec.name, i.spec.risk).is_some())
            })
            .collect();
        out.sort_by(|a, b| a.spec.name.cmp(&b.spec.name));
        out
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
        let approval = match self.gate(m.session, &m.tool, m.risk, false) {
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
            .filter(|spec| {
                !session.is_some_and(|s| self.capped(s, &spec.name, spec.risk).is_some())
            })
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
        self.validators.remove(&name);
        self.tools.insert(name, m.0);
    }
}

impl Message<UnregisterTool> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: UnregisterTool, _: &mut Context<Self, Self::Reply>) {
        if self.tools.remove(&m.0).is_some() {
            self.validators.remove(&m.0);
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

impl Message<RegisterFenceChecker> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: RegisterFenceChecker, _: &mut Context<Self, Self::Reply>) {
        tracing::info!(lang = %m.lang, "registered fence checker");
        self.fence_checkers.insert(m.lang, m.checker);
    }
}

/// Runs off the actor: a checker may spend seconds in a helper process and
/// the registry must keep serving tool calls meanwhile.
impl Message<CheckFence> for ToolRegistry {
    type Reply = DelegatedReply<Option<Result<Vec<FenceError>, String>>>;

    async fn handle(&mut self, m: CheckFence, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let Some(checker) = self.fence_checkers.get(&m.lang).cloned() else {
            tx.send(None);
            return reply;
        };
        tokio::spawn(async move {
            tx.send(Some(checker.check(&m.body).await));
        });
        reply
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
    type Reply = Vec<(&'static str, String)>;

    async fn handle(
        &mut self,
        _: ListPrompts,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<(&'static str, String)> {
        self.prompts.iter().map(|(p, b)| (*p, b.clone())).collect()
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
        let Some(tool) = self.visible(call.session, &call.name) else {
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
        let mentions = mentions(&tool, &call.args);
        let approval = match self.gate(call.session, &call.name, spec.risk, mentions) {
            Ok(a) => a,
            Err(why) => {
                tx.send(ToolOutput::err(format!("{}: {why}", call.name)));
                return reply;
            }
        };
        self.dispatch(tool, spec, call, approval, admitted, move |out| {
            tx.send(out)
        });
        reply
    }
}

/// The gate's secret input: a placeholder in `args` counts only for a tool
/// that consumes it ([`crate::ErasedTool::consumes_secrets`]); for one that
/// stores its arguments as data the placeholder is the data.
fn mentions(tool: &DynTool, args: &serde_json::Value) -> bool {
    tool.consumes_secrets() && crate::secrets::mentions(args)
}

/// The binding an app holds a tool to: sha256 hex over the canonical JSON
/// of `{name, schema, origin}` ([`crate::ErasedTool::origin`]). Private so
/// nothing outside this actor can recompute it from display data. Object
/// keys are sorted at every depth before hashing (arrays keep their order,
/// `required`/`enum` are positional): the unified feature graph turns on
/// `serde_json/preserve_order`, so an MCP server re-sending the same schema
/// with keys in a different order must still hash equal.
fn identity(tool: &DynTool, spec: &ToolSpec) -> String {
    use sha2::{Digest as _, Sha256};
    let mut canon = serde_json::json!({
        "name": spec.name,
        "schema": spec.schema,
        "origin": tool.origin(),
    });
    canon.sort_all_objects();
    format!("{:x}", Sha256::digest(canon.to_string()))
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
    resources: Arc<SessionResources>,
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
    // A tool that stores its arguments as an artifact keeps them literal.
    if tool.consumes_secrets()
        && let Err(e) = crate::secrets::expand(&mut call.args)
    {
        return ToolOutput::err(e);
    }
    let cx = ToolCx {
        session: call.session,
        call_id: call.id,
        quiet: call.quiet,
        resources,
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

/// Drop a registered name whose actor is no longer alive: an actor whose
/// runtime went away is aborted without its `on_stop` unregistration, and
/// the stale entry would otherwise refuse every later `start` in the process
/// (test binaries run one runtime per test). A live actor is never displaced.
fn evict_dead<A: kameo::Actor>(name: &'static str) {
    if ActorRef::<A>::lookup(name)
        .ok()
        .flatten()
        .is_some_and(|a| !a.is_alive())
    {
        kameo::registry::ACTOR_REGISTRY
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(name);
    }
}

/// Spawn registry, approval and jobs actors and register builtins. Returns the
/// registry so the caller can attach further tool sources (MCP).
pub async fn start(rt: &Runtime) -> anyhow::Result<ActorRef<ToolRegistry>> {
    let cfg = config(rt)?;
    evict_dead::<Approval>(APPROVAL_ACTOR);
    evict_dead::<AskUser>(ASK_ACTOR);
    evict_dead::<Jobs>(JOBS_ACTOR);
    evict_dead::<ToolRegistry>(REGISTRY_ACTOR);

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

    let shell = shell::ShellTool::new(jobs.clone(), cfg.login_path);
    let shells = shell.shells();
    let pty_idle = Arc::new(AtomicU64::new(cfg.pty_idle_secs));
    let builtins: [DynTool; 14] = [
        Arc::new(shell),
        Arc::new(pty::StartTool::new(
            jobs.clone(),
            Arc::clone(&shells),
            Duration::from_secs(cfg.pty_ready_timeout_secs),
            Arc::clone(&pty_idle),
            cfg.pty_shell.clone(),
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
        Arc::new(glob::Glob),
        Arc::new(edit::Edit),
        Arc::new(AskUserTool(ask)),
    ];
    let registry = ToolRegistry::spawn_with_mailbox(
        ToolRegistry {
            tools: builtins.into_iter().map(|t| (t.spec().name, t)).collect(),
            validators: HashMap::new(),
            prompts: BTreeMap::new(),
            fence_checkers: HashMap::new(),
            policy: cfg.policy,
            approvals: cfg.approvals,
            bypass: HashSet::new(),
            hidden: HashMap::new(),
            parents: HashMap::new(),
            cap: HashMap::new(),
            config_path: rt.data_dir.join("config.yaml"),
            resources: HashMap::new(),
            durable: SessionResources::durable_arc(&rt.data_dir),
            approval: None,
            activity: None,
            untracked: Arc::default(),
            communication: None,
            gate: None,
            participants: Vec::new(),
            expected: Vec::new(),
            shells,
            close_timeout: CLOSE_TIMEOUT,
            pty_idle,
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

    fn registry_with(
        tools: Vec<DynTool>,
        activity: Option<ActivitySink>,
        policy: Policy,
        approval: Option<ActorRef<Approval>>,
    ) -> ActorRef<ToolRegistry> {
        ToolRegistry::spawn(ToolRegistry {
            policy,
            approval,
            ..build(tools, activity)
        })
    }

    fn build(tools: Vec<DynTool>, activity: Option<ActivitySink>) -> ToolRegistry {
        ToolRegistry {
            tools: tools.into_iter().map(|t| (t.spec().name, t)).collect(),
            validators: HashMap::new(),
            prompts: BTreeMap::new(),
            fence_checkers: HashMap::new(),
            policy: Policy::default(),
            approvals: true,
            bypass: HashSet::new(),
            hidden: HashMap::new(),
            parents: HashMap::new(),
            cap: HashMap::new(),
            config_path: PathBuf::new(),
            resources: HashMap::new(),
            durable: SessionResources::durable_arc(std::path::Path::new("/tmp")),
            approval: None,
            activity,
            untracked: Arc::default(),
            communication: None,
            gate: None,
            participants: Vec::new(),
            expected: Vec::new(),
            shells: Arc::default(),
            close_timeout: Duration::from_millis(100),
            pty_idle: Arc::default(),
        }
    }

    fn registry(tools: Vec<DynTool>, activity: Option<ActivitySink>) -> ActorRef<ToolRegistry> {
        ToolRegistry::spawn(build(tools, activity))
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

    /// Takes its arguments as data: a placeholder is literal text.
    struct Literal;

    #[async_trait::async_trait]
    impl Tool for Literal {
        const NAME: &'static str = "literal";
        const RISK: Risk = Risk::ReadOnly;
        const CONSUMES_SECRETS: bool = false;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, a: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok(a["q"].as_str().unwrap_or_default()))
        }
    }

    #[tokio::test]
    async fn placeholder_in_literal_args_neither_raises_the_gate_nor_expands() {
        let reg = registry(vec![Arc::new(Echo), Arc::new(Literal)], None);
        let session = SessionId::new();
        let with = |name: &str| ToolCall {
            args: serde_json::json!({"q": "find {{secret:GITHUB}} usage"}),
            ..call(name, session)
        };
        // No approval actor: a raised gate is an error, not a prompt.
        let out = reg.ask(Invoke(with("echo"))).await.unwrap();
        assert!(out.is_error && out.content.contains("approval"), "{out:?}");
        let out = reg.ask(Invoke(with("literal"))).await.unwrap();
        assert!(!out.is_error, "{out:?}");
        assert_eq!(out.content, "find {{secret:GITHUB}} usage");
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

    /// A `Mutating` tool for the cap tests; changes nothing.
    struct Touch;

    #[async_trait::async_trait]
    impl Tool for Touch {
        const NAME: &'static str = "touch";
        const RISK: Risk = Risk::Mutating;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok("touched"))
        }
    }

    /// A capped session (or a descendant) is not offered a tool above the
    /// cap and is refused with the cap's own text when it names one; the
    /// tightest ancestor cap wins; read-only tools and `send_message` are
    /// untouched; clearing the cap restores everything.
    #[tokio::test]
    async fn cap_hides_and_denies_tools_above_it_down_the_tree() {
        let allow = Policy {
            read_only: PolicyAction::Allow,
            mutating: PolicyAction::Allow,
            destructive: PolicyAction::Allow,
        };
        let reg = registry_with(
            vec![Arc::new(Echo), Arc::new(Touch), Arc::new(Peer(Arc::default()))],
            None,
            allow,
            None,
        );
        let (parent, child, grandchild) = (SessionId::new(), SessionId::new(), SessionId::new());
        for (session, parent) in [(child, parent), (grandchild, child)] {
            reg.tell(SetParent { session, parent }).await.unwrap();
        }
        let names = |specs: Vec<ToolSpec>| specs.into_iter().map(|s| s.name).collect::<Vec<_>>();

        // No cap: the mutating tool is listed and runs.
        assert_eq!(
            names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo", "touch"]
        );
        assert!(!reg.ask(Invoke(call("touch", child))).await.unwrap().is_error);

        // (a) capped at read_only: `touch` is not offered ...
        reg.tell(SetCap {
            session: child,
            cap: Some(Risk::ReadOnly),
        })
        .await
        .unwrap();
        assert_eq!(
            names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo"]
        );
        // (b) ... and naming it anyway is the cap's refusal, not the policy's.
        let out = reg.ask(Invoke(call("touch", child))).await.unwrap();
        assert!(out.is_error, "{out:?}");
        assert_eq!(out.content, "touch: denied: subagent is capped at read_only");
        // (e) a read-only tool still runs.
        assert!(!reg.ask(Invoke(call("echo", child))).await.unwrap().is_error);
        // `send_message` is exempt: still described, still allowed.
        let described = reg.ask(DescribeTools(Some(child))).await.unwrap();
        let peer = described
            .iter()
            .find(|i| i.spec.name == "send_message")
            .expect("send_message survives the cap");
        assert_eq!(peer.action, PolicyAction::Allow);
        assert!(described.iter().all(|i| i.spec.name != "touch"));
        // The parent is untouched.
        assert_eq!(
            names(reg.ask(ListTools(Some(parent))).await.unwrap()),
            ["echo", "touch"]
        );

        // (c) the grandchild inherits the tighter ancestor cap even when its
        // own is looser.
        reg.tell(SetCap {
            session: grandchild,
            cap: Some(Risk::Destructive),
        })
        .await
        .unwrap();
        assert_eq!(
            names(reg.ask(ListTools(Some(grandchild))).await.unwrap()),
            ["echo"]
        );
        let out = reg.ask(Invoke(call("touch", grandchild))).await.unwrap();
        assert_eq!(out.content, "touch: denied: subagent is capped at read_only");

        // (d) clearing the child's cap restores both.
        reg.tell(SetCap {
            session: child,
            cap: None,
        })
        .await
        .unwrap();
        assert_eq!(
            names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo", "touch"]
        );
        assert!(!reg.ask(Invoke(call("touch", grandchild))).await.unwrap().is_error);
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

    /// Destructive; counts runs and echoes the args it received.
    struct Wipe(Arc<std::sync::atomic::AtomicUsize>);

    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct WipeArgs {
        target: String,
    }

    #[async_trait::async_trait]
    impl Tool for Wipe {
        const NAME: &'static str = "wipe";
        const RISK: Risk = Risk::Destructive;
        type Args = WipeArgs;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, a: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolOutput::ok(a.target))
        }
    }

    fn press(request_id: &str) -> Option<UserActivation> {
        Some(UserActivation {
            record_call_id: "rec".into(),
            action_id: "act".into(),
            request_id: request_id.into(),
        })
    }

    async fn bound(reg: &ActorRef<ToolRegistry>, session: SessionId, name: &str) -> String {
        reg.ask(InspectTool {
            session,
            name: name.into(),
            args: None,
        })
        .await
        .unwrap()
        .identity
    }

    fn app_call(name: &str, s: SessionId, args: serde_json::Value) -> ToolCall {
        ToolCall {
            args,
            ..call(name, s)
        }
    }

    #[tokio::test]
    async fn press_answers_ask_for_destructive_and_secret_calls_but_not_deny() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reg = registry(vec![Arc::new(Wipe(runs.clone())), Arc::new(Echo)], None);
        let session = SessionId::new();
        let wipe = bound(&reg, session, "wipe").await;
        let echo = bound(&reg, session, "echo").await;

        // No approval actor is running: a model call that reaches `Ask` fails
        // right there, which is how we know the press never went near it.
        let model = reg
            .ask(Invoke(app_call(
                "wipe",
                session,
                serde_json::json!({"target": "db"}),
            )))
            .await
            .unwrap();
        assert!(
            model.is_error && model.content.contains("approval required"),
            "{model:?}"
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);

        let pressed = reg
            .ask(AppInvoke {
                call: app_call("wipe", session, serde_json::json!({"target": "db"})),
                activation: press("1"),
                expected_identity: wipe.clone(),
            })
            .await
            .unwrap();
        assert!(
            matches!(&pressed, AppInvoked::Completed(o) if !o.is_error && o.content == "db"),
            "{pressed:?}"
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);

        // A read-only tool whose args mention a secret is `Ask` too; the
        // press covers it, and the placeholder stays a placeholder until the
        // (absent) vault would expand it at the tool boundary.
        let secret = serde_json::json!({"token": "{{secret:API}}"});
        assert!(matches!(
            reg.ask(AppInvoke {
                call: app_call("echo", session, secret.clone()),
                activation: None,
                expected_identity: echo.clone(),
            })
            .await
            .unwrap(),
            AppInvoked::NeedsApproval
        ));
        assert!(matches!(
            reg.ask(AppInvoke {
                call: app_call("echo", session, secret),
                activation: press("1"),
                expected_identity: echo,
            })
            .await
            .unwrap(),
            AppInvoked::Completed(o) if !o.is_error
        ));

        // `Deny` is final: the press changes nothing and the tool never runs.
        let mut denying = build(vec![Arc::new(Wipe(runs.clone()))], None);
        denying.policy.destructive = PolicyAction::Deny;
        let denying = ToolRegistry::spawn(denying);
        let out = denying
            .ask(AppInvoke {
                call: app_call("wipe", session, serde_json::json!({"target": "db"})),
                activation: press("1"),
                expected_identity: wipe,
            })
            .await
            .unwrap();
        assert!(
            matches!(&out, AppInvoked::Unavailable(why) if why.contains("denied by policy")),
            "{out:?}"
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn automatic_ask_needs_approval_and_never_prompts_or_runs() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reg = registry(vec![Arc::new(Wipe(runs.clone()))], None);
        let session = SessionId::new();
        let wipe = bound(&reg, session, "wipe").await;
        for _ in 0..2 {
            let out = reg
                .ask(AppInvoke {
                    call: app_call("wipe", session, serde_json::json!({"target": "db"})),
                    activation: None,
                    expected_identity: wipe.clone(),
                })
                .await
                .unwrap();
            assert!(matches!(out, AppInvoked::NeedsApproval), "{out:?}");
        }
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
        // The inspection says so up front, so the app can draw a Load button.
        let i = reg
            .ask(InspectTool {
                session,
                name: "wipe".into(),
                args: Some(serde_json::json!({"target": "db"})),
            })
            .await
            .unwrap();
        assert_eq!(i.action, PolicyAction::Ask);
        assert_eq!(i.source, ToolSource::native());
        // Policy is not part of the identity; the switches never mutate it.
        reg.ask(SetApprovals {
            session: Some(session),
            enabled: false,
        })
        .await
        .unwrap();
        assert_eq!(bound(&reg, session, "wipe").await, wipe);
        assert_eq!(
            reg.ask(DescribeTools(Some(session)))
                .await
                .unwrap()
                .into_iter()
                .map(|i| (i.spec.name, i.identity))
                .collect::<Vec<_>>(),
            [("wipe".to_owned(), wipe)]
        );
    }

    #[tokio::test]
    async fn stale_binding_bad_args_or_foreign_activation_never_dispatch() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reg = registry(vec![Arc::new(Wipe(runs.clone()))], None);
        let session = SessionId::new();
        let wipe = bound(&reg, session, "wipe").await;
        let go = |args, activation, expected: &str| {
            reg.ask(AppInvoke {
                call: app_call("wipe", session, args),
                activation,
                expected_identity: expected.to_owned(),
            })
        };
        let ok = serde_json::json!({"target": "db"});

        let out = go(ok.clone(), press("1"), "0000").await.unwrap();
        assert!(
            matches!(&out, AppInvoked::Unavailable(w) if w.contains("identity changed")),
            "{out:?}"
        );
        let out = go(serde_json::json!({"target": 7}), press("1"), &wipe)
            .await
            .unwrap();
        assert!(
            matches!(&out, AppInvoked::Unavailable(w) if w.contains("invalid arguments")),
            "{out:?}"
        );
        let out = go(ok.clone(), press("other"), &wipe).await.unwrap();
        assert!(
            matches!(&out, AppInvoked::Unavailable(w) if w.contains("covers request")),
            "{out:?}"
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);

        // Hidden for the session: unavailable, same as the model's `unknown tool`.
        reg.ask(SetHidden {
            session,
            tools: vec!["wipe".into()],
            hidden: true,
        })
        .await
        .unwrap();
        assert!(matches!(
            go(ok, press("1"), &wipe).await.unwrap(),
            AppInvoked::Unavailable(_)
        ));
        assert!(
            reg.ask(InspectTool {
                session,
                name: "wipe".into(),
                args: None
            })
            .await
            .is_err()
        );
    }

    /// Stands in for an MCP tool: the schema is whatever the server sent.
    struct Raw(serde_json::Value);

    impl crate::ErasedTool for Raw {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "raw".into(),
                description: String::new(),
                schema: self.0.clone(),
                risk: Risk::ReadOnly,
                direct: false,
                returns: String::new(),
            }
        }
        fn call(
            &self,
            _: serde_json::Value,
            _: ToolCx,
        ) -> futures::future::BoxFuture<'_, anyhow::Result<ToolOutput>> {
            Box::pin(async { Ok(ToolOutput::ok("")) })
        }
    }

    #[test]
    fn identity_ignores_object_key_order_but_not_schema_or_array_order() {
        // Built by insertion so `preserve_order` (on in the unified feature
        // graph) keeps the two orders distinct in memory.
        let obj = |pairs: &[(&str, serde_json::Value)]| {
            let mut m = serde_json::Map::new();
            for (k, v) in pairs {
                m.insert((*k).to_owned(), v.clone());
            }
            serde_json::Value::Object(m)
        };
        let target = |ty: &str| obj(&[("type", ty.into()), ("minLength", 1.into())]);
        let a = obj(&[
            ("type", "object".into()),
            (
                "properties",
                obj(&[
                    ("target", target("string")),
                    ("force", obj(&[("type", "boolean".into())])),
                ]),
            ),
            ("required", serde_json::json!(["target", "force"])),
        ]);
        let b = obj(&[
            ("required", serde_json::json!(["target", "force"])),
            (
                "properties",
                obj(&[
                    ("force", obj(&[("type", "boolean".into())])),
                    (
                        "target",
                        obj(&[("minLength", 1.into()), ("type", "string".into())]),
                    ),
                ]),
            ),
            ("type", "object".into()),
        ]);
        let id = |schema: serde_json::Value| {
            let tool: DynTool = Arc::new(Raw(schema));
            let spec = tool.spec();
            identity(&tool, &spec)
        };
        assert_eq!(id(a.clone()), id(b));
        // Semantic changes still rebind: a different property type, and a
        // reordered `required` array (position is meaning there).
        let mut changed = a.clone();
        changed["properties"]["target"]["type"] = "integer".into();
        assert_ne!(id(a.clone()), id(changed));
        let mut swapped = a.clone();
        swapped["required"] = serde_json::json!(["force", "target"]);
        assert_ne!(id(a), id(swapped));
    }

    #[tokio::test]
    async fn concurrent_model_call_still_asks_while_a_press_runs() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let approval = Approval::spawn(Approval::new(rt, Duration::from_millis(20)));
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut r = build(vec![Arc::new(Wipe(runs.clone()))], None);
        r.approval = Some(approval);
        let reg = ToolRegistry::spawn(r);
        let session = SessionId::new();
        let wipe = bound(&reg, session, "wipe").await;

        let model = tokio::spawn({
            let reg = reg.clone();
            async move {
                reg.ask(Invoke(app_call(
                    "wipe",
                    session,
                    serde_json::json!({"target": "model"}),
                )))
                .await
                .unwrap()
            }
        });
        let pressed = reg
            .ask(AppInvoke {
                call: app_call("wipe", session, serde_json::json!({"target": "press"})),
                activation: press("1"),
                expected_identity: wipe,
            })
            .await
            .unwrap();
        assert!(
            matches!(&pressed, AppInvoked::Completed(o) if o.content == "press"),
            "{pressed:?}"
        );
        // Nobody answered the model's prompt: it timed out, i.e. it was asked.
        let model = model.await.unwrap();
        assert!(
            model.is_error && model.content.contains("approval timed out"),
            "{model:?}"
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A lifecycle owner: records what it was told opened; on close, acks
    /// under its name or, when `hang`, never answers.
    #[derive(kameo::Actor)]
    struct Participant {
        owner: &'static str,
        hang: bool,
        opened: Arc<Mutex<Vec<TreeOpened>>>,
    }

    impl Message<TreeOpened> for Participant {
        type Reply = ();
        async fn handle(&mut self, m: TreeOpened, _: &mut Context<Self, Self::Reply>) {
            lock(&self.opened).push(m);
        }
    }

    impl Message<CloseTree> for Participant {
        type Reply = CloseAck;
        async fn handle(&mut self, _: CloseTree, _: &mut Context<Self, Self::Reply>) -> CloseAck {
            if self.hang {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            CloseAck::ok(self.owner)
        }
    }

    /// Reports the root the registry resolved for the call.
    struct Root;

    #[async_trait::async_trait]
    impl Tool for Root {
        const NAME: &'static str = "root";
        const RISK: Risk = Risk::ReadOnly;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, _: Self::Args, cx: ToolCx) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok(cx.resources.root.display().to_string()))
        }
    }

    #[tokio::test]
    async fn lifecycle_barrier_waits_for_expected_and_collects_acks() {
        let reg = registry(vec![Arc::new(Root)], None);
        reg.ask(ExpectParticipants(vec!["x", "y"])).await.unwrap();
        let Err(kameo::error::SendError::HandlerError(missing)) = reg.ask(ParticipantsReady).await
        else {
            panic!("ready before anyone registered");
        };
        assert_eq!(missing, "x, y");

        let opened = Arc::new(Mutex::new(Vec::new()));
        let x = Participant::spawn(Participant {
            owner: "x",
            hang: false,
            opened: Arc::clone(&opened),
        });
        reg.ask(RegisterLifecycle {
            owner: "x",
            opened: Some(x.clone().recipient()),
            close: x.reply_recipient(),
        })
        .await
        .unwrap();
        assert!(matches!(
            reg.ask(ParticipantsReady).await,
            Err(kameo::error::SendError::HandlerError(m)) if m == "y"
        ));
        let y = Participant::spawn(Participant {
            owner: "y",
            hang: true,
            opened: Arc::default(),
        });
        reg.ask(RegisterLifecycle {
            owner: "y",
            opened: None,
            close: y.reply_recipient(),
        })
        .await
        .unwrap();
        reg.ask(ParticipantsReady).await.unwrap();

        let root = SessionId::new();
        let child = SessionId::new();
        let resources = Arc::new(SessionResources {
            root: PathBuf::from("/tmp/private-root"),
            retention: crate::Retention::Private,
            generation: 1,
        });
        reg.ask(OpenTree(TreeOpened {
            root,
            sessions: vec![root, child],
            resources: Arc::clone(&resources),
        }))
        .await
        .unwrap();
        let mut shown = None;
        for _ in 0..50 {
            if lock(&opened).len() == 1 {
                shown = Some(());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(shown.is_some(), "x was not told the tree opened");
        assert_eq!(lock(&opened)[0].sessions, [root, child]);
        // Both sessions resolve to the installed root until the close.
        let root_of = |s: SessionId| {
            let reg = reg.clone();
            async move { reg.ask(Invoke(call("root", s))).await.unwrap().content }
        };
        assert_eq!(root_of(child).await, "/tmp/private-root");

        let acks = reg
            .ask(CloseTreeAll(CloseTree {
                subtree: vec![root, child],
                generation: 1,
                cancel: tokio_util::sync::CancellationToken::new(),
            }))
            .await
            .unwrap();
        let by = |o: &str| {
            acks.iter()
                .find(|a| a.owner == o)
                .unwrap_or_else(|| panic!("{o}"))
        };
        assert!(by("x").ok);
        assert!(!by("y").ok);
        assert_eq!(by("y").detail.as_deref(), Some("timeout"));
        assert!(!by("jobs").ok, "no jobs actor in this test");
        assert_eq!(acks.len(), 3);
        assert_eq!(root_of(root).await, "/tmp");
        assert_eq!(root_of(child).await, "/tmp");
    }

    struct Nag;

    #[async_trait::async_trait]
    impl FenceChecker for Nag {
        async fn check(&self, body: &str) -> Result<Vec<FenceError>, String> {
            Ok(if body.contains("bad") {
                vec![FenceError {
                    path: "line:1".into(),
                    message: "bad".into(),
                }]
            } else {
                vec![]
            })
        }
    }

    #[tokio::test]
    async fn fence_checker_is_looked_up_by_language() {
        let reg = registry(vec![], None);
        let check = |lang: &str, body: &str| {
            reg.ask(CheckFence {
                lang: lang.into(),
                body: body.into(),
            })
        };
        assert!(check("mermaid", "bad").await.unwrap().is_none());
        reg.ask(RegisterFenceChecker {
            lang: "mermaid".into(),
            checker: Arc::new(Nag),
        })
        .await
        .unwrap();
        assert_eq!(check("mermaid", "good").await.unwrap(), Some(Ok(vec![])));
        let errs = check("mermaid", "bad").await.unwrap().unwrap().unwrap();
        assert_eq!(errs[0].path, "line:1");
        assert!(check("openui", "bad").await.unwrap().is_none());
    }
}
