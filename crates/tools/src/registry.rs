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
use tokio_util::sync::CancellationToken;

use crate::activity::{self, Activity, ActivitySink, Kind, OperationId};
use crate::approval::{Approval, Decision, RequestApproval, RequestHumanOnly};
use crate::ask::{AskUser, AskUserTool};
use crate::builtins::{self, BUILTIN_CATEGORY_PREFIX, BuiltinCategory, BuiltinTool, CATEGORIES};
use crate::categories::{
    self, CustomCategoryDef, TOOL_CATEGORY_PREFIX, ToolCategoriesConfig, ToolCategoryInfo,
};
use crate::communication::{self, Admitted, CommunicationAccess};
use crate::jobs::{CloseSessions, InstallActivity, JOBS_ACTOR, Jobs, lock};
use crate::journal::{InstallJournal, Journal, NoopJournal, QuestionRow};
use crate::resources::{CloseAck, CloseTree, SessionResources, TreeOpened};
use crate::{
    APPROVAL_ACTOR, ASK_ACTOR, DynTool, Policy, PolicyAction, PreviewCx, REGISTRY_ACTOR, Risk,
    ToolCall, ToolCx, ToolOutput, ToolSource, ToolSpec, ToolsConfig, edit, fs, glob, jobs, pty,
    reminder, schema, shell,
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
    /// 183 §4.5 (W-4): sessions' approval pins (`sessions.approvals`): `true`
    /// keeps `Ask` an `Ask` even with `tools.approvals: false`, `false` turns
    /// it into `Allow`; no entry inherits `tools.approvals`. Subagents
    /// inherit the nearest ancestor's pin through `parents`.
    pin: HashMap<SessionId, bool>,
    /// Tool names a session (and its subagents) does not see: absent from
    /// [`ListTools`], `unknown tool` on [`Invoke`]. An [`mcp_server_key`]
    /// entry hides every tool of that MCP server, including tools it registers
    /// later. The persistent switch is `off` in `mcp/config.yaml`.
    hidden: HashMap<SessionId, HashSet<String>>,
    /// Bumped whenever a [`SetHidden`] or [`RestoreHidden`] changes a hide set;
    /// a running turn compares it before each model call ([`HiddenEpoch`]).
    hidden_epoch: u64,
    /// 206: the harness-wide builtin switch (`tools.builtins.disabled`): builtin
    /// names and [`builtins::category_key`]s hidden for every session. Seeded at
    /// [`start`], changed by [`SetBuiltinsDisabled`]; never touches MCP tools.
    builtins_disabled: HashSet<String>,
    /// High-level tool categories and their builtin/MCP assignments.
    categories: ToolCategoriesConfig,
    /// 219 §3.1: the harness-wide always-ask set (`tools.always_ask`): tool names and
    /// [`mcp_server_key`]s whose calls always raise a human card. Seeded at [`start`],
    /// changed by [`SetAlwaysAsk`] with no session.
    always_ask: HashSet<String>,
    /// 219 §3.2: each chat's own always-ask set (`sessions.always_ask`); subagents
    /// inherit their ancestors' through `parents`.
    always_ask_session: HashMap<SessionId, HashSet<String>>,
    /// 219 §3.2: sessions whose stored `sessions.always_ask` could not be read at restore
    /// ([`AlwaysAskUnreadable`]). Their entries are unknown, so every `Mutating`-or-above call
    /// in them and their subagents is always ask until a [`RestoreAlwaysAsk`] reads the row.
    /// Only that failure path sets this; no entry a client can write means "everything".
    always_ask_unreadable: HashSet<SessionId>,
    /// child → parent, from [`SetParent`]; a session inherits its ancestors' pin and hidden set.
    parents: HashMap<SessionId, SessionId>,
    /// Risk ceiling per session from [`SetCap`]: a tool above it is neither
    /// listed for nor runnable by the session or its subagents, except
    /// [`CAP_EXEMPT`]. In memory only, like `hidden`.
    cap: HashMap<SessionId, Risk>,
    /// 183 §14.6 (W-39): a contact's turn in an exported chat, from [`SetTurnCap`]; read
    /// beside `cap` (the stricter wins, down the tree), never written into it.
    turn_cap: HashMap<SessionId, Risk>,
    /// 183 §7.6 (Z3-R2): contact sessions are default-deny. A session (a contact's inbox) or
    /// a contact's turn (in an exported chat) with an allowlist here sees only the tools
    /// named in it; every other tool is hidden, for it and its subagents. In memory only,
    /// re-installed at every actor start of a contact session.
    allow_only: HashMap<SessionId, HashSet<String>>,
    /// The turn-scoped allowlist of a contact's turn, from [`SetAllowOnly`] with `turn`.
    turn_allow_only: HashMap<SessionId, HashSet<String>>,
    /// 16 §5 (2026-11-11): sessions a plugin runs a tool call under without the local-network
    /// grant it holds itself (a notebook source's ingest job), from [`SetNoLan`].
    no_lan: HashSet<SessionId>,
    /// 210 §7.2: sessions a chat channel bound to a thread whose user is not the owner, from
    /// [`SetNonOwnerThread`]. A human-only tool is refused in them and their subagents. In
    /// memory only; the channels re-send it for their bound threads at start.
    non_owner_thread: HashSet<SessionId>,
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
    /// The MCP allowlist source (67 §6.3), from [`InstallMcpAccess`]. `None`:
    /// every MCP tool is offered to every session.
    mcp_access: Option<Arc<dyn McpAccess>>,
    /// The caller's execution bound (50 §8), from [`InstallExecutionGate`];
    /// `None`: waits are not reported to anyone.
    gate: Option<Arc<dyn ExecutionGate>>,
    /// 109 §3.3 step 6: the journal seam, from [`InstallJournal`]; a
    /// [`NoopJournal`] until the agent installs its store. [`invoke`] marks
    /// every call `started` / `finished` through it.
    journal: Arc<dyn Journal>,
    /// Lifecycle owners from [`RegisterLifecycle`] (51 §4.4), by name; a
    /// second registration under a name replaces the first.
    participants: Vec<Participant>,
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
    /// 145 §0.4: per-session promotion delay from [`SetPromoteAfter`]; a
    /// session absent here never promotes. Read by [`Invoke`] only.
    promote_after: HashMap<SessionId, Duration>,
    /// 145 §0.5: the [`ToolCx::cancel`] of every inline call still running,
    /// by `(session, call_id)`; [`CancelCalls`] fires them. A promoted call
    /// leaves the map (its token is the job's from then on).
    inflight: Arc<Mutex<HashMap<(SessionId, String), CancellationToken>>>,
}

/// One [`RegisterLifecycle`] entry: owner name, its `TreeOpened` recipient, its `CloseTree` reply recipient.
type Participant = (
    &'static str,
    Option<Recipient<TreeOpened>>,
    ReplyRecipient<CloseTree, CloseAck>,
);

/// `ask` → `Vec<ToolSpec>` for every registered tool, sorted by name; with a
/// session, minus what [`SetHidden`] hid for it or an ancestor.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListTools(pub Option<SessionId>);

/// `ask` → `()`. Hide (or show again) tools for one session; the owner writes
/// the session's set to `sessions.hidden_tools` (the journal) before it replies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetHidden {
    /// Session to switch (its subagents follow).
    pub session: SessionId,
    /// Registered tool names, [`mcp_server_key`]s for whole MCP servers, or
    /// [`builtins::category_key`]s for whole builtin categories (206).
    pub tools: Vec<String>,
    /// `true` hides.
    pub hidden: bool,
}

/// Prefix of a [`SetHidden`] entry naming a whole MCP server (see [`mcp_server_key`]).
pub const MCP_SERVER_HIDE_PREFIX: &str = "mcp:";

/// The [`SetHidden`] entry that hides every tool of MCP server `server` for a
/// session and its subagents — tools the server registers later (reconnect,
/// restart) included. MCP tool names cannot contain `:`, so it matches no tool.
pub fn mcp_server_key(server: &str) -> String {
    format!("{MCP_SERVER_HIDE_PREFIX}{server}")
}

/// `ask` → `u64`: the registry's hide generation. It changes whenever a
/// session's hide set does, so a running turn re-lists its tools (prompt index,
/// runtime function set, direct tools) before its next model call.
#[derive(Clone, Copy, Debug)]
pub struct HiddenEpoch;

/// `ask` → `HashSet<String>`: what [`SetHidden`] hid for the session or an ancestor.
#[derive(Clone, Copy, Debug)]
pub struct HiddenTools(pub SessionId);

/// `ask` → `()`: replace a session's own hidden set with the list loaded from
/// its `sessions.hidden_tools` row (actor spawn: boot, resume, take) without
/// writing it back. Engine-internal. Names of tools not registered (removed
/// since) stay inert: hiding is by name, so they match nothing.
#[derive(Clone, Debug)]
pub struct RestoreHidden {
    /// The session.
    pub session: SessionId,
    /// The row's names; empty clears any set held in memory.
    pub tools: Vec<String>,
}

/// `ask` → `Vec<BuiltinCategory>` (206): every registered builtin (`Native`
/// source) by category, in [`CATEGORIES`] order, tools sorted by name; a
/// category with no registered tool is left out. With a session, `hidden`
/// reports its (and its ancestors') hide set.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListBuiltins(pub Option<SessionId>);

/// `ask` → `Result<(), BuiltinsSwitchError>` (206): add (`disabled`) or remove
/// names in the harness-wide builtin switch. The new set is written to
/// `tools.builtins.disabled` first and only then applied, so an error leaves
/// memory and file as they were. A call that changes nothing writes nothing and
/// leaves [`HiddenEpoch`] alone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetBuiltinsDisabled {
    /// Builtin tool names and [`builtins::category_key`]s. Adding: each must be a
    /// registered builtin or a known category key. Removing: also any name in the
    /// set (a stale entry of a tool no longer registered).
    pub names: Vec<String>,
    /// `true` switches them off for every session.
    pub disabled: bool,
}

/// Why [`SetBuiltinsDisabled`] changed nothing (206).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuiltinsSwitchError {
    /// Names that are neither a registered builtin nor a known category key
    /// (nor, when removing, in the set).
    Unknown(Vec<String>),
    /// `config.yaml` could not be written: the path and the reason.
    NotWritten(String),
}

impl std::fmt::Display for BuiltinsSwitchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(names) => write!(
                f,
                "not a builtin tool or category key: {}",
                names.join(", ")
            ),
            Self::NotWritten(why) => write!(
                f,
                "the config file could not be written, nothing changed: {why}"
            ),
        }
    }
}

/// `ask` → every high-level tool category, defaults first in display order and
/// custom categories sorted by label. With a session, `hidden` includes ancestors.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListToolCategories(pub Option<SessionId>);

/// `ask` → the `tcat:<id>` hides a brand-new root interactive session starts with.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultHiddenCategories;

/// `ask` → create or replace a custom category, persisted before it is applied.
#[derive(Clone, Debug)]
pub struct UpsertToolCategory {
    /// Existing custom id to update, a new explicit id, or `None` to slugify `label`.
    pub id: Option<String>,
    /// Display name; leading and trailing whitespace is trimmed.
    pub label: String,
    /// One-line summary; `None` stores an empty description.
    pub description: Option<String>,
}

/// `ask` → delete a custom category and every assignment/default referring to it.
#[derive(Clone, Debug)]
pub struct DeleteToolCategory {
    /// Custom category id to delete.
    pub id: String,
}

/// `ask` → assign an MCP server to a category, or clear its assignment with `None`.
#[derive(Clone, Debug)]
pub struct SetMcpCategory {
    /// MCP server name.
    pub server: String,
    /// Existing default or custom category id, or `None` to clear.
    pub category: Option<String>,
}

/// `ask` → override a native tool's default category, or clear the override.
#[derive(Clone, Debug)]
pub struct SetToolCategory {
    /// Registered native tool name (or a stale name when clearing).
    pub tool: String,
    /// Existing default or custom category id, or `None` to clear.
    pub category: Option<String>,
}

/// `ask` → set the categories hidden in brand-new root interactive sessions.
#[derive(Clone, Debug)]
pub struct SetDefaultToolCategories {
    /// Existing category ids; sorted and deduplicated before persistence.
    pub disabled: Vec<String>,
}

/// 219 §2.1: tools that run inside the agent loop and never pass the registry, so an
/// always-ask entry for them could never fire: `python`, `todo`, `set_status`, `advise`,
/// `consult` and the endless tools. [`SetAlwaysAsk`] refuses to add them.
pub const LOOP_LOCAL_TOOLS: &[&str] = &[
    "python",
    "todo",
    "set_status",
    "advise",
    "consult",
    "zoom",
    "recall",
    "date",
    "ledger",
];

/// 219 §2.4: the tools that (re)register MCP servers. Re-adding a server under another name
/// would escape its `mcp:<server>` and `<server>__<tool>` entries, so while any always-ask
/// entry applies to a session these calls are always ask there too.
pub const MCP_ADMIN_TOOLS: &[&str] = &["add_mcp_server", "remove_mcp_server", "add_mcp_repo"];

/// `ask` → `Result<AlwaysAskState, AlwaysAskError>` (219 §4): add (`on`) or remove one
/// always-ask entry, harness-wide (`session: None`, persisted to `tools.always_ask` before
/// it is applied) or for one chat and its subagents (written through the journal). An
/// entry is a registered tool name or an [`mcp_server_key`]. Setting the current state
/// changes nothing and writes nothing. No human check here: that is the RPC layer's (§2.4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetAlwaysAsk {
    /// The chat, or `None` for the harness-wide set.
    pub session: Option<SessionId>,
    /// Tool name or [`mcp_server_key`].
    pub tool: String,
    /// `true` adds the entry.
    pub on: bool,
}

/// `ask` → `()`: replace a session's own always-ask set with the list loaded from its
/// `sessions.always_ask` row (actor spawn) without writing it back. Empty clears it.
#[derive(Clone, Debug)]
pub struct RestoreAlwaysAsk {
    /// The session.
    pub session: SessionId,
    /// The row's names.
    pub names: Vec<String>,
}

/// `ask` → `()`: the session's `sessions.always_ask` row could not be read or parsed at
/// restore (219 §3.2). Fail closed: every `Mutating`-or-above call in the session and its
/// subagents is always ask until a [`RestoreAlwaysAsk`] for it succeeds.
#[derive(Clone, Copy, Debug)]
pub struct AlwaysAskUnreadable {
    /// The session whose row failed.
    pub session: SessionId,
}

/// `ask` → [`AlwaysAskState`]: the harness-wide set and, with a session, its own and
/// its ancestors' entries.
#[derive(Clone, Copy, Debug, Default)]
pub struct GetAlwaysAsk {
    /// The chat asked about; `None` reports the harness-wide set only.
    pub session: Option<SessionId>,
}

/// 219 §4: the always-ask entries that apply, by where they come from.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AlwaysAskState {
    /// `tools.always_ask`, sorted.
    pub global: Vec<String>,
    /// This chat's own entries, sorted; empty for a global query.
    pub session: Vec<String>,
    /// Entries from ancestor chats not in `session`, sorted.
    pub inherited: Vec<String>,
}

/// Why [`SetAlwaysAsk`] changed nothing (219 §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlwaysAskError {
    /// A loop-local tool ([`LOOP_LOCAL_TOOLS`]) cannot be marked.
    LoopLocal(String),
    /// The entry could not be stored: `config.yaml not written: <err>` (harness-wide) or
    /// `sessions.always_ask not written: <err>` (a chat), shown after `always_ask: `.
    NotWritten(String),
    /// The tool name is empty.
    Empty,
}

impl std::fmt::Display for AlwaysAskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoopLocal(name) => write!(
                f,
                "always_ask: {name} runs inside the agent loop and is never gated; mark the runtime functions it calls instead"
            ),
            Self::NotWritten(why) => write!(f, "always_ask: {why}"),
            Self::Empty => f.write_str("always_ask: tool name is empty"),
        }
    }
}

impl std::error::Error for AlwaysAskError {}

/// `ask` → [`ToolOutput`] (206): [`Invoke`] for a call the harness makes itself (a
/// notebook read, an overview's `show`, a Settings test), not a model. It skips only
/// the builtin switch ([`ToolRegistry::builtin_switched`]); hides by name or MCP
/// server, allowlists, caps, policy and approvals apply as for [`Invoke`]. Never for
/// a call made on a model's behalf.
#[derive(Debug)]
pub struct HarnessInvoke(pub ToolCall);

/// `ask` → [`ToolOutput`]. Never fails: unknown tool, denied approval, bad
/// args, tool panic and timeout all come back as `ToolOutput { is_error: true }`.
#[derive(Debug)]
pub struct Invoke(pub ToolCall);

/// `ask` → [`ToolOutput`] (216 §10.3): [`Invoke`] for a call an external plugin brokers
/// (`call`), never the model's. Visibility, policy and the tool's own risk apply as for
/// [`Invoke`]; when the policy asks, the card is a [`RequestHumanOnly`] (the double and channel
/// users cannot answer it) whose args gain `requested_by: "<requester> wants to run <tool>"`.
/// Timeout is the ordinary one and denies. Never fails.
#[derive(Debug)]
pub struct InvokeHumanOnly {
    /// The call, in the plugin's headless session.
    pub call: ToolCall,
    /// Who asks, as the card names it (`plugin turns`).
    pub requester: String,
}

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

/// `ask` → `Option<ToolSource>`: who registered `name`, `None` when nothing is. 216 §8: a plugin
/// tool never replaces a native, MCP or other plugin's tool, and unregisters only its own names.
pub struct ToolSourceOf(pub String);

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

/// Per-session MCP allowlist (67 §6.3): the roster plugin installs it; the
/// registry asks it, for the session that lists or calls, whether an MCP
/// tool of `server` named `tool` (the server's own name) is offered. A tool
/// it refuses is hidden exactly like a [`SetHidden`] one. Sessions the
/// source does not know are unaffected (`true`).
#[async_trait::async_trait]
pub trait McpAccess: Send + Sync {
    /// May `session` see `server`'s `tool`?
    fn allows(&self, session: SessionId, server: &str, tool: &str) -> bool;

    /// Persist access for the agent whose approved `add_mcp_server` just connected
    /// `server`. Unrestricted sessions need no grant. Must update the read model
    /// before returning; other visibility and risk gates still apply.
    async fn grant(&self, session: SessionId, server: &str) -> Result<(), String>;
}

/// `ask` → `()`: install the [`McpAccess`] source; installing again replaces it.
pub struct InstallMcpAccess(pub Arc<dyn McpAccess>);

impl Message<InstallMcpAccess> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: InstallMcpAccess, _: &mut Context<Self, Self::Reply>) {
        self.mcp_access = Some(m.0);
    }
}

/// Clone the source so granting can await the roster outside this mailbox.
pub(crate) struct GetMcpAccess;

impl Message<GetMcpAccess> for ToolRegistry {
    type Reply = Option<Arc<dyn McpAccess>>;

    async fn handle(&mut self, _: GetMcpAccess, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.mcp_access.clone()
    }
}

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

/// `ask` → [`ExternalVerdict`]: the same gate as [`Invoke`] for a tool call
/// theuth does not run itself — an external harness's
/// `session/request_permission` (design doc 06 §3.4, §8.8). Timeout and a
/// missing approval actor are `Deny`.
#[derive(Clone, Debug)]
pub struct ExternalApproval {
    /// Session the call belongs to (approval pin inheritance).
    pub session: SessionId,
    /// Tool title, shown in the modal.
    pub tool: String,
    /// Raw arguments, shown in the modal.
    pub args: serde_json::Value,
    /// Risk class the caller derived (`acp::risk(kind)`).
    pub risk: Risk,
}

/// The answer to an [`ExternalApproval`]. `dismissed` (207): the user left the
/// call to the agent from Attention — not run, but not a refusal either; the
/// caller must not report it as one. `decision` is then `Deny`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, kameo::Reply)]
pub struct ExternalVerdict {
    /// Run it or not.
    pub decision: Decision,
    /// Not run because it was left to the agent, not refused.
    pub dismissed: bool,
}

impl ExternalVerdict {
    /// A plain allow / deny (gate, policy, timeout, unreachable approver).
    pub const fn plain(decision: Decision) -> Self {
        Self {
            decision,
            dismissed: false,
        }
    }
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
/// session (pinned on its row, 183 §4.5) or, with `session: None`, for
/// everyone, persisted to `tools.approvals` in `<data_dir>/config.yaml`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetApprovals {
    /// Session to switch; `None` switches the global default.
    pub session: Option<SessionId>,
    /// `true` prompts as the policy says; `false` never prompts.
    pub enabled: bool,
    /// 183 §4.5 (W-4): drop the session's pin (`sessions.approvals` back to
    /// NULL, inherit `tools.approvals`); `enabled` is then ignored.
    #[serde(default)]
    pub clear: bool,
}

/// `ask` → `()`: apply a session's pin loaded from its `sessions.approvals`
/// row (actor spawn: boot, resume, take) without writing it back. Engine-internal.
#[derive(Clone, Copy, Debug)]
pub struct RestoreApprovals {
    /// The session.
    pub session: SessionId,
    /// The row's pin; `None` clears any pin held in memory.
    pub pin: Option<bool>,
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
/// the parent's approval switch (and its [`crate::seat`]).
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
        crate::seat::inherit(m.session, m.parent);
    }
}

/// `ask` → `()`: record the stored parent of an ancestor whose actor is not spawned here
/// (219 §3.2), so a resumed subagent's inheritance walk reaches past it before that ancestor
/// spawns. Only the link: no [`crate::seat`] is inherited (the ancestor's own spawn seats it
/// from its row), and a link already known is kept.
#[derive(Clone, Copy, Debug)]
pub struct LinkAncestor {
    /// The ancestor session.
    pub session: SessionId,
    /// Its stored parent.
    pub parent: SessionId,
}

impl Message<LinkAncestor> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: LinkAncestor, _: &mut Context<Self, Self::Reply>) {
        self.parents.entry(m.session).or_insert(m.parent);
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

/// `tell` (183 §14.6, W-39): a turn-scoped ceiling beside [`SetCap`] for a contact's turn in
/// an exported chat; the stricter of the two wins in `capped`, and the session's own
/// [`SetCap`] entry is never overwritten or cleared. `None` ends it (`TurnFinished`, cancel).
#[derive(Clone, Copy, Debug)]
pub struct SetTurnCap {
    /// The exported chat.
    pub session: SessionId,
    /// The contact's cap; `None` removes the turn cap.
    pub cap: Option<Risk>,
}

impl Message<SetTurnCap> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetTurnCap, _: &mut Context<Self, Self::Reply>) {
        match m.cap {
            Some(cap) => self.turn_cap.insert(m.session, cap),
            None => self.turn_cap.remove(&m.session),
        };
    }
}

/// `tell` (145 §0.4): inline calls of `session` whose body has not returned
/// after `after` are promoted to background jobs ([`crate::Adopt`]); `None`
/// never promotes. The agent sets it from `TalkLive.promote_after_ms` and
/// clears it when the talk ends. Not inherited by subagents.
#[derive(Clone, Copy, Debug)]
pub struct SetPromoteAfter {
    /// The talk's session.
    pub session: SessionId,
    /// Promotion delay; `None` clears it.
    pub after: Option<Duration>,
}

impl Message<SetPromoteAfter> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetPromoteAfter, _: &mut Context<Self, Self::Reply>) {
        match m.after {
            Some(after) => self.promote_after.insert(m.session, after),
            None => self.promote_after.remove(&m.session),
        };
    }
}

/// `tell` (145 §0.5): the turn running `session`'s inline calls was
/// cancelled — fire the [`ToolCx::cancel`] of every call still in flight,
/// before the caller writes its synthetic results. Promoted jobs are not
/// touched (their token is `cancel_job`'s).
#[derive(Clone, Copy, Debug)]
pub struct CancelCalls {
    /// The cancelled turn's session.
    pub session: SessionId,
}

impl Message<CancelCalls> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: CancelCalls, _: &mut Context<Self, Self::Reply>) {
        for ((s, _), token) in lock(&self.inflight).iter() {
            if *s == m.session {
                token.cancel();
            }
        }
    }
}

/// An MCP tool's name on its server: the registered name, less `<server>__` after a clash.
fn mcp_remote<'a>(name: &'a str, server: &str) -> &'a str {
    name.strip_prefix(server)
        .and_then(|rest| rest.strip_prefix("__"))
        .unwrap_or(name)
}

/// Tools a capped subagent keeps whatever its ceiling: reporting to its
/// parent is what a subagent is for, and `send_message` is `Mutating` for
/// the approval policy (a peer's inbox changes), not because it touches
/// anything the cap protects. The policy still runs for these.
const CAP_EXEMPT: &[&str] = &["send_message"];

/// `tell` (183 §7.6, Z3-R2): contact sessions are default-deny — no tool exists for `session`
/// (and its subagents) but the reply; everything else is hidden. `turn` scopes it to
/// the running contact turn of an exported chat (cleared at turn end), otherwise it is the
/// session's own (a contact's inbox). `None` clears that entry. 191 (D5): only the empty
/// list is installed; a non-empty `tools` is installed as the empty list and logged.
#[derive(Clone, Debug)]
pub struct SetAllowOnly {
    /// The inbox, or the exported chat whose contact turn runs.
    pub session: SessionId,
    /// The allowlist; `None` removes it.
    pub tools: Option<Vec<String>>,
    /// Turn-scoped (an exported chat's contact turn) rather than the session's own.
    pub turn: bool,
}

impl Message<SetAllowOnly> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetAllowOnly, _: &mut Context<Self, Self::Reply>) {
        let map = if m.turn {
            &mut self.turn_allow_only
        } else {
            &mut self.allow_only
        };
        match m.tools {
            // 191 (D5): every caller passes the empty list; a contact's session has no tool but
            // the reply. A non-empty one is installed as the empty list (never skipped: a
            // session with no entry drops out of `contact_scoped`; never installed as given).
            Some(t) => {
                if !t.is_empty() {
                    tracing::warn!(session = %m.session, tools = ?t, "a contact allowlist must be empty; installed empty");
                }
                map.insert(m.session, HashSet::new())
            }
            None => map.remove(&m.session),
        };
    }
}

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
/// `resources`/parent/hidden/pin/cap entries go with it.
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
            self.always_ask_session.remove(s);
            self.always_ask_unreadable.remove(s);
            self.pin.remove(s);
            self.cap.remove(s);
            self.turn_cap.remove(s);
            self.allow_only.remove(s);
            self.turn_allow_only.remove(s);
            self.no_lan.remove(s);
            self.non_owner_thread.remove(s);
            crate::seat::forget(*s);
            crate::textref::forget_clips(*s);
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
    /// Whether the session prompts: its (or the nearest ancestor's) pin, else
    /// `tools.approvals`; meaningful only for a session query.
    pub session: bool,
    /// 183 §4.5 (W-4): the session's own pin (`sessions.approvals`): `Some(true)`
    /// = `'on'`, `Some(false)` = `'off'`, `None` = inherit (or a global query).
    /// Absent when `None`, as the TS `pin?` says (a `null` read as "off").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub pin: Option<bool>,
}

impl Message<SetApprovals> for ToolRegistry {
    type Reply = ApprovalsState;

    async fn handle(
        &mut self,
        m: SetApprovals,
        _: &mut Context<Self, Self::Reply>,
    ) -> ApprovalsState {
        match m.session {
            // 183 §4.5 (W-4): the session's own pin, written through to its row
            // before the reply, so a reader that re-applies the row sees it.
            Some(id) => {
                let pin = (!m.clear).then_some(m.enabled);
                self.set_pin(id, pin);
                self.journal.approvals(id, pin).await;
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

impl Message<RestoreApprovals> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: RestoreApprovals, _: &mut Context<Self, Self::Reply>) {
        self.set_pin(m.session, m.pin);
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
    /// Set (`Some`) or drop (`None`, inherit) the session's own pin.
    fn set_pin(&mut self, id: SessionId, pin: Option<bool>) {
        match pin {
            Some(on) => self.pin.insert(id, on),
            None => self.pin.remove(&id),
        };
    }

    /// The global switch and, for a session, its effective value (pin, else global).
    fn approvals_state(&self, session: Option<SessionId>) -> ApprovalsState {
        ApprovalsState {
            global: self.approvals,
            session: session.map_or(self.approvals, |id| {
                self.pinned(id).unwrap_or(self.approvals)
            }),
            pin: session.and_then(|id| self.pin.get(&id).copied()),
        }
    }

    /// The nearest pin of the session or an ancestor (183 §4.5).
    fn pinned(&self, mut id: SessionId) -> Option<bool> {
        // ponytail: linear walk per invoke; depth is capped by `max_depth`.
        loop {
            if let Some(&on) = self.pin.get(&id) {
                return Some(on);
            }
            id = *self.parents.get(&id)?;
        }
    }

    /// `name` is hidden for the session or an ancestor (by name, or its MCP server by
    /// [`mcp_server_key`]), is an MCP tool the session's agent may not see (67 §6.3), is
    /// outside a contact session's allowlist (183 Z3-R2), or is a builtin switched off by
    /// a builtin switch (206: [`Self::builtin_switched`]).
    fn is_hidden(&self, id: SessionId, name: &str) -> bool {
        self.builtin_switched(id, name) || self.hidden_apart_from_switch(id, name)
    }

    /// [`Self::is_hidden`] without the builtin switch: what a [`HarnessInvoke`] honours.
    fn hidden_apart_from_switch(&self, mut id: SessionId, name: &str) -> bool {
        if self.mcp_hidden(id, name) || !self.allowlist_admits(id, name) {
            return true;
        }
        // The server key, resolved on the first set that names any server.
        let mut server: Option<Option<String>> = None;
        // MCP category key, resolved only when a hide set names a tool category.
        let mut category: Option<Option<String>> = None;
        loop {
            if let Some(h) = self.hidden.get(&id) {
                if h.contains(name) {
                    return true;
                }
                if h.iter().any(|k| k.starts_with(MCP_SERVER_HIDE_PREFIX)) {
                    let key = server.get_or_insert_with(|| {
                        self.tools.get(name).and_then(|t| match t.source() {
                            ToolSource::Mcp { server, .. } => Some(mcp_server_key(&server)),
                            ToolSource::Native { .. } => None,
                            // 216 §8: a plugin's tools sit under the reserved server key
                            // `plugin.<name>` (the MCP loader refuses servers named `plugin.*`).
                            ToolSource::Plugin { name, .. } => {
                                Some(mcp_server_key(&crate::mcp::plugin_server_key(&name)))
                            }
                        })
                    });
                    if key.as_ref().is_some_and(|k| h.contains(k)) {
                        return true;
                    }
                }
                if h.iter().any(|k| k.starts_with(TOOL_CATEGORY_PREFIX)) {
                    let key = category.get_or_insert_with(|| {
                        self.tools.get(name).and_then(|t| match t.source() {
                            ToolSource::Mcp { .. } => self
                                .effective_category_of(name)
                                .map(|id| categories::tool_category_key(&id)),
                            // 216: plugin tools carry no tool category in v1.
                            ToolSource::Native { .. } | ToolSource::Plugin { .. } => None,
                        })
                    });
                    if key.as_ref().is_some_and(|k| h.contains(k)) {
                        return true;
                    }
                }
            }
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return false,
            }
        }
    }

    /// 206: `name` is a registered builtin (its source is `Native`).
    fn is_native(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .is_some_and(|t| matches!(t.source(), ToolSource::Native { .. }))
    }

    /// The effective high-level category: native tools default by builtin group and
    /// may be overridden; MCP tools inherit their server's explicit assignment.
    fn effective_category_of(&self, name: &str) -> Option<String> {
        match self.tools.get(name)?.source() {
            ToolSource::Native { .. } => Some(
                self.categories
                    .tool_assignments
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| categories::default_category_for_builtin(name).to_owned()),
            ),
            ToolSource::Mcp { server, .. } => self.categories.mcp_assignments.get(&server).cloned(),
            // 216: plugin tools carry no tool category in v1.
            ToolSource::Plugin { .. } => None,
        }
    }

    /// 206: builtin `name` is off by a builtin switch: its name or category key is in the
    /// harness-wide set, or its category key is in the hidden set of `id` or an ancestor.
    /// A hide by plain name is [`Self::is_hidden`]'s own business. MCP tools: never.
    fn builtin_switched(&self, mut id: SessionId, name: &str) -> bool {
        let mut key: Option<Option<String>> = None;
        let mut key = || {
            key.get_or_insert_with(|| {
                self.is_native(name)
                    .then(|| builtins::category_key_of(name))
            })
            .clone()
        };
        let mut tool_category: Option<Option<String>> = None;
        let mut tool_category = || {
            tool_category
                .get_or_insert_with(|| {
                    self.tools.get(name).and_then(|tool| match tool.source() {
                        ToolSource::Native { .. } => self
                            .effective_category_of(name)
                            .map(|id| categories::tool_category_key(&id)),
                        ToolSource::Mcp { .. } | ToolSource::Plugin { .. } => None,
                    })
                })
                .clone()
        };
        if !self.builtins_disabled.is_empty()
            && let Some(k) = key()
            && (self.builtins_disabled.contains(name) || self.builtins_disabled.contains(&k))
        {
            return true;
        }
        loop {
            if let Some(h) = self.hidden.get(&id) {
                if h.iter().any(|k| k.starts_with(BUILTIN_CATEGORY_PREFIX))
                    && key().is_some_and(|k| h.contains(&k))
                {
                    return true;
                }
                // Kept in this switch, not `hidden_apart_from_switch`: harness-owned
                // invocations deliberately bypass category hides for native tools.
                if h.iter().any(|k| k.starts_with(TOOL_CATEGORY_PREFIX))
                    && tool_category().is_some_and(|k| h.contains(&k))
                {
                    return true;
                }
            }
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return false,
            }
        }
    }

    /// 206: the hidden sets of `id` and its ancestors, nearest first.
    fn hidden_chain(&self, mut id: SessionId) -> Vec<&HashSet<String>> {
        let mut sets = Vec::new();
        loop {
            sets.extend(self.hidden.get(&id));
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return sets,
            }
        }
    }

    fn category_exists(&self, id: &str) -> bool {
        categories::default_category(id).is_some()
            || self.categories.custom.iter().any(|c| c.id == id)
    }

    /// Native tools and assigned MCP servers grouped by effective category. The
    /// maps are ordered, and native tool vectors are sorted when a DTO is built.
    fn category_members(&self) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
        let mut native: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, tool) in &self.tools {
            if !matches!(tool.source(), ToolSource::Native { .. }) {
                continue;
            }
            if let Some(category) = self.effective_category_of(name) {
                native.entry(category).or_default().push(name.clone());
            }
        }
        let mut mcp: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (server, category) in &self.categories.mcp_assignments {
            mcp.entry(category.clone())
                .or_default()
                .push(server.clone());
        }
        (native, mcp)
    }

    fn category_info(
        &self,
        definition: (&str, &str, &str),
        custom: bool,
        hidden_chain: &[&HashSet<String>],
        native_members: &mut BTreeMap<String, Vec<String>>,
        mcp_members: &mut BTreeMap<String, Vec<String>>,
    ) -> ToolCategoryInfo {
        let (id, label, description) = definition;
        let mut builtin_tools = native_members.remove(id).unwrap_or_default();
        builtin_tools.sort();
        let key = categories::tool_category_key(id);
        ToolCategoryInfo {
            id: id.to_owned(),
            label: label.to_owned(),
            description: description.to_owned(),
            custom,
            default_enabled: !self.categories.default_disabled.iter().any(|d| d == id),
            hidden: hidden_chain.iter().any(|set| set.contains(&key)),
            builtin_count: builtin_tools.len() as u32,
            builtin_tools,
            mcp_servers: mcp_members.remove(id).unwrap_or_default(),
        }
    }

    /// Serialize as JSON flow syntax (also YAML) and persist before memory changes.
    fn persist_categories<T: Serialize + ?Sized>(
        &self,
        field: &str,
        value: &T,
    ) -> Result<(), String> {
        let value = serde_json::to_string(value)
            .map_err(|e| format!("could not serialize tools.categories.{field}: {e}"))?;
        runtime::write_nested_config_value(&self.config_path, "tools", "categories", field, &value)
            .map_err(|e| format!("{} could not be written: {e}", self.config_path.display()))
    }

    /// 219 §2.2: `name` (or its MCP server's [`mcp_server_key`]) is in the harness-wide
    /// always-ask set, or in the set of `session` or an ancestor (the walk of
    /// [`Self::hidden_chain`]). While any entry applies, the [`MCP_ADMIN_TOOLS`] are always
    /// ask too (§2.4), so re-registering a server cannot shed its marks.
    fn always_ask_applies(&self, session: Option<SessionId>, name: &str) -> bool {
        let mut sets: Vec<&HashSet<String>> = vec![&self.always_ask];
        let mut cur = session;
        while let Some(id) = cur {
            sets.extend(self.always_ask_session.get(&id));
            cur = self.parents.get(&id).copied();
        }
        if sets.iter().all(|s| s.is_empty()) {
            return false;
        }
        if MCP_ADMIN_TOOLS.contains(&name) || sets.iter().any(|s| s.contains(name)) {
            return true;
        }
        let server = self.tools.get(name).and_then(|t| match t.source() {
            ToolSource::Mcp { server, .. } => Some(mcp_server_key(&server)),
            // 216: a plugin's tools sit under its `plugin.<name>` server key.
            ToolSource::Plugin { name, .. } => Some(mcp_server_key(&crate::mcp::plugin_server_key(&name))),
            ToolSource::Native { .. } => None,
        });
        server.is_some_and(|k| sets.iter().any(|s| s.contains(&k)))
    }

    /// [`GetAlwaysAsk`]'s answer.
    fn always_ask_state(&self, session: Option<SessionId>) -> AlwaysAskState {
        let Some(id) = session else {
            return AlwaysAskState {
                global: sorted(&self.always_ask),
                ..AlwaysAskState::default()
            };
        };
        let own = self.always_ask_session.get(&id);
        let mut inherited = HashSet::new();
        let mut cur = self.parents.get(&id).copied();
        while let Some(p) = cur {
            if let Some(set) = self.always_ask_session.get(&p) {
                inherited.extend(
                    set.iter()
                        .filter(|n| !own.is_some_and(|o| o.contains(*n)))
                        .cloned(),
                );
            }
            cur = self.parents.get(&p).copied();
        }
        AlwaysAskState {
            global: sorted(&self.always_ask),
            session: own.map(sorted).unwrap_or_default(),
            inherited: sorted(&inherited),
        }
    }

    /// 219 §2.4: a built-in tool call that writes `<data_dir>/config.yaml` or
    /// `<data_dir>/mcp/config.yaml` is always ask. The written paths are read from the
    /// argument names the file tools use (`path`; `before`/`after`/`at` refs, cut at `#`;
    /// `move_text`'s `src`, which it deletes from), resolved like the tools do (relative to
    /// the seated cwd; `~` too, though the tools do not expand it) and compared after
    /// canonicalising what exists. Only `Native` tools at `Mutating` or above count: MCP
    /// servers' paths are their own business, and a read-only tool writes nothing.
    ///
    /// The native tools that write a caller-chosen path (audited for 219 m5) and the names
    /// that carry it: `write_file`, `edit`, `indent` (`path`); `move_text` (`before`/`after`/
    /// `at`, `src`); `copy_text` (`before`/`after`/`at`); `doc_export` (`dest`). Every other
    /// native writer takes only a name inside a fixed directory. A new path-writing tool must
    /// use one of these names or extend the lists.
    fn protected_write(
        &self,
        session: SessionId,
        name: &str,
        risk: Risk,
        args: &serde_json::Value,
    ) -> bool {
        const PLAIN: &[&str] = &["path", "dest", "destination", "to", "file", "out", "output"];
        const REFS: &[&str] = &["before", "after", "at"];
        if risk < Risk::Mutating || !self.is_native(name) {
            return false;
        }
        let Some(data_dir) = self
            .config_path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
        else {
            return false;
        };
        let Some(obj) = args.as_object() else {
            return false;
        };
        let protected = [
            self.config_path.clone(),
            data_dir.join("mcp").join("config.yaml"),
        ];
        let protected_real: Vec<Option<PathBuf>> =
            protected.iter().map(|p| crate::seat::real(p)).collect();
        let hits = |raw: &str| {
            let raw = raw.trim();
            if raw.is_empty() {
                return false;
            }
            let expanded = match raw.strip_prefix("~") {
                Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
                    match std::env::home_dir() {
                        Some(home) => home.join(rest.trim_start_matches(['/', '\\'])),
                        None => PathBuf::from(raw),
                    }
                }
                _ => PathBuf::from(raw),
            };
            let path = crate::seat::resolve(session, &expanded);
            let real = crate::seat::real(&path);
            protected.iter().zip(&protected_real).any(|(p, pr)| {
                let same = |a: &std::path::Path, b: &std::path::Path| {
                    a == b
                        || (cfg!(any(target_os = "macos", windows))
                            && a.as_os_str()
                                .to_string_lossy()
                                .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy()))
                };
                same(&path, p)
                    || match (&real, pr) {
                        (Some(a), Some(b)) => same(a, b),
                        (Some(a), None) => same(a, p),
                        _ => false,
                    }
            })
        };
        let plain = PLAIN.iter().filter_map(|k| obj.get(*k)?.as_str());
        let refs = REFS
            .iter()
            .chain((name == "move_text").then_some(&"src"))
            .filter_map(|k| obj.get(*k)?.as_str())
            .map(|r| r.split_once('#').map_or(r, |(p, _)| p));
        plain.chain(refs).any(hits)
    }

    /// 206: a name [`SetBuiltinsDisabled`] accepts: a registered builtin or the key of
    /// a known category.
    fn is_builtin_name(&self, name: &str) -> bool {
        match name.strip_prefix(BUILTIN_CATEGORY_PREFIX) {
            Some(id) => builtins::category(id).is_some(),
            None => self.is_native(name),
        }
    }

    /// 183 Z3-R2: every allowlist on the session or an ancestor lists `name`, and so does a
    /// contact turn's allowlist on the session itself (Z3-R3: not on the chat's existing
    /// subagents, which keep the owner's tools). None applies: admitted. 191 (D5): every
    /// allowlist is empty (see [`SetAllowOnly`]), so under one nothing is admitted.
    fn allowlist_admits(&self, id: SessionId, name: &str) -> bool {
        let admits = |allowed: &HashSet<String>| allowed.contains(name);
        if self.turn_allow_only.get(&id).is_some_and(|a| !admits(a)) {
            return false;
        }
        let mut cur = id;
        loop {
            if self.allow_only.get(&cur).is_some_and(|a| !admits(a)) {
                return false;
            }
            match self.parents.get(&cur) {
                Some(p) => cur = *p,
                None => return true,
            }
        }
    }

    /// 183 Z3-R2: the session is a contact session: it, or an ancestor, is an inbox, or a
    /// contact's turn runs in the session itself.
    fn contact_scoped(&self, id: SessionId) -> bool {
        if self.turn_allow_only.contains_key(&id) {
            return true;
        }
        let mut cur = id;
        loop {
            if self.allow_only.contains_key(&cur) {
                return true;
            }
            match self.parents.get(&cur) {
                Some(p) => cur = *p,
                None => return false,
            }
        }
    }

    /// 16 §5 (2026-11-11): a call of `id` may reach the local network unless `id` is in a
    /// restricted (capped) contact state ([`Self::contact_scoped`]: the public web only) or a
    /// plugin ran it without the grant ([`SetNoLan`]). A full-share contact turn or full inbox
    /// runs with the agent's own tools and ceiling (190), so the LAN follows the setting there
    /// like in the owner's own turns (Sky, 2026-11-11 20:28).
    fn lan_allowed(&self, id: SessionId) -> bool {
        !self.contact_scoped(id) && !self.no_lan.contains(&id)
    }

    /// 67 §6.3: an MCP tool outside the allowlist of the agent `id` works for.
    /// The source resolves descendants itself (it learns of every child), so
    /// one question per session suffices. The registered name is the server's
    /// tool name, or `<server>__<name>` after a clash (`mcp.rs`).
    fn mcp_hidden(&self, id: SessionId, name: &str) -> bool {
        let Some(access) = &self.mcp_access else {
            return false;
        };
        let Some(tool) = self.tools.get(name) else {
            return false;
        };
        let ToolSource::Mcp { server, .. } = tool.source() else {
            return false;
        };
        let remote = mcp_remote(name, &server);
        !access.allows(id, &server, remote)
    }

    /// The tightest cap on `id`: its own or an ancestor's [`SetCap`], and a contact turn's
    /// [`SetTurnCap`] on `id` itself (183 Z3-R3: not on the chat's running subagents, which
    /// the owner started and which keep the owner's tools).
    fn cap_for(&self, id: SessionId) -> Option<Risk> {
        let mut cap = self.turn_cap.get(&id).copied();
        let mut cur = id;
        loop {
            if let Some(c) = self.cap.get(&cur) {
                cap = Some(cap.map_or(*c, |k| k.min(*c)));
            }
            match self.parents.get(&cur) {
                Some(p) => cur = *p,
                None => return cap,
            }
        }
    }

    /// The cap that puts `name` at `risk` out of the session's reach, if any:
    /// `risk` above [`Self::cap_for`] and the tool not in [`CAP_EXEMPT`] — which never
    /// applies in a contact session (183 Z3-R2: a contact's cap has no relay exemption).
    fn capped(&self, id: SessionId, name: &str, risk: Risk) -> Option<Risk> {
        self.cap_for(id)
            .filter(|cap| risk > *cap && (!CAP_EXEMPT.contains(&name) || self.contact_scoped(id)))
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
    /// hidden for the session or an ancestor. `harness` (a [`HarnessInvoke`]):
    /// the builtin switch does not apply.
    fn visible(&self, session: SessionId, name: &str, harness: bool) -> Option<DynTool> {
        let hidden = || {
            if harness {
                self.hidden_apart_from_switch(session, name)
            } else {
                self.is_hidden(session, name)
            }
        };
        self.tools.get(name).filter(|_| !hidden()).cloned()
    }

    /// The one permission decision (12 §5), before anyone is asked: `Deny`
    /// for a tool above the session's cap ([`SetCap`]), else `tools.policy`
    /// for the risk (for a tool whose card is the approval,
    /// [`crate::ErasedTool::approved_by_card`], the policy of `ReadOnly`: 209 §9),
    /// at least `Ask` when the args carry a secret placeholder
    /// (37 §0.4), then `Ask` → `Allow` unless prompts are on for the session:
    /// its nearest pin ([`SetApprovals`], 183 §4.5), else `tools.approvals`.
    /// `Deny` is never rewritten. A [`crate::Tool::HUMAN_ONLY`] tool (210 §7.2) is `Deny` in a
    /// contact turn or a session bound to a non-owner chat thread, else at least `Ask`, and
    /// neither the session pin nor `tools.approvals: false` rewrites that `Ask`. 219 §2.2: so
    /// is an always-ask call ([`Self::always`]; `protected` is [`Self::protected_write`] of
    /// the call's args).
    fn effective(
        &self,
        session: Option<SessionId>,
        name: &str,
        risk: Risk,
        mentions: bool,
        protected: bool,
    ) -> PolicyAction {
        if session.is_some_and(|s| self.capped(s, name, risk).is_some()) {
            return PolicyAction::Deny;
        }
        let tool = self.tools.get(name);
        let human_only = tool.is_some_and(|t| t.human_only());
        let always = self.always(session, name, risk, protected);
        if (human_only || always) && session.is_some_and(|s| self.owner_absent(s)) {
            return PolicyAction::Deny;
        }
        let by_card = tool.is_some_and(|t| t.approved_by_card());
        let mut action = self
            .policy
            .action(if by_card { Risk::ReadOnly } else { risk });
        if mentions {
            action = action.max(PolicyAction::Ask);
        }
        if human_only || always {
            return action.max(PolicyAction::Ask);
        }
        if action == PolicyAction::Ask
            && !session
                .and_then(|s| self.pinned(s))
                .unwrap_or(self.approvals)
        {
            action = PolicyAction::Allow;
        }
        action
    }

    /// 219 §2.2: the call is always ask: the tool is marked ([`Self::always_ask_applies`]),
    /// the call writes a protected config file (`protected`), or it is `Mutating` or above in
    /// a session whose (or whose ancestor's) stored entries could not be read
    /// ([`AlwaysAskUnreadable`]).
    fn always(&self, session: Option<SessionId>, name: &str, risk: Risk, protected: bool) -> bool {
        protected
            || self.always_ask_applies(session, name)
            || (risk >= Risk::Mutating && session.is_some_and(|s| self.always_ask_unknown(s)))
    }

    /// `id` or an ancestor is in [`Self::always_ask_unreadable`].
    fn always_ask_unknown(&self, mut id: SessionId) -> bool {
        if self.always_ask_unreadable.is_empty() {
            return false;
        }
        loop {
            if self.always_ask_unreadable.contains(&id) {
                return true;
            }
            match self.parents.get(&id) {
                Some(p) => id = *p,
                None => return false,
            }
        }
    }

    /// 210 §7.2: the owner is not the one in this session: a contact turn or inbox
    /// ([`Self::contact_scoped`]), or a session (or an ancestor) bound to a non-owner chat
    /// thread ([`SetNonOwnerThread`]).
    fn owner_absent(&self, id: SessionId) -> bool {
        if self.contact_scoped(id) {
            return true;
        }
        let mut cur = id;
        loop {
            if self.non_owner_thread.contains(&cur) {
                return true;
            }
            match self.parents.get(&cur) {
                Some(p) => cur = *p,
                None => return false,
            }
        }
    }

    /// Whether `name` is a registered [`crate::Tool::HUMAN_ONLY`] tool.
    fn human_only(&self, name: &str) -> bool {
        self.tools.get(name).is_some_and(|t| t.human_only())
    }

    /// Why a `Deny` from [`Self::effective`] happened, for the model: the cap
    /// names its ceiling so the subagent knows the refusal is not the policy's.
    fn denied(&self, session: SessionId, name: &str, risk: Risk, protected: bool) -> String {
        match self.capped(session, name, risk) {
            Some(cap) => format!("denied: subagent is capped at {cap}"),
            None if self.human_only(name) && self.owner_absent(session) => {
                "denied: only the owner can approve this, and this conversation is not the owner's (a contact's turn or a non-owner chat thread)".to_owned()
            }
            None if self.always(Some(session), name, risk, protected) && self.owner_absent(session) => {
                format!(
                    "always ask: {name} needs the owner's approval and the owner is not in this conversation"
                )
            }
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
        protected: bool,
    ) -> Result<Option<ActorRef<Approval>>, String> {
        match self.effective(Some(session), name, risk, mentions, protected) {
            PolicyAction::Allow => Ok(None),
            PolicyAction::Deny => Err(self.denied(session, name, risk, protected)),
            PolicyAction::Ask => self.approval_actor().map(Some),
        }
    }

    /// The approval actor, looked up at first need.
    fn approval_actor(&mut self) -> Result<ActorRef<Approval>, String> {
        if self.approval.is_none() {
            self.approval = ActorRef::<Approval>::lookup(APPROVAL_ACTOR).ok().flatten();
        }
        self.approval
            .clone()
            .ok_or_else(|| "approval required but no approval actor is running".to_owned())
    }

    /// Spec, origin, identity and effective action of one tool; `risk` is
    /// the call's risk when args were given ([`call_risk`]), else the spec's;
    /// `protected` is [`Self::protected_write`] of those args.
    fn inspect(
        &self,
        tool: &DynTool,
        session: Option<SessionId>,
        mentions: bool,
        risk: Option<Risk>,
        protected: bool,
    ) -> ToolInspection {
        let mut spec = tool.spec();
        if let Some(risk) = risk {
            spec.risk = risk;
        }
        ToolInspection {
            action: self.effective(session, &spec.name, spec.risk, mentions, protected),
            identity: identity(tool, &spec),
            source: tool.source(),
            spec,
        }
    }

    /// Everything an app call must pass before it is dispatched like any
    /// other (46 §6.3): visibility, communication availability (50 §0.5),
    /// the bound identity, the current schema, that the activation is for
    /// *this* call, then policy. `Err` is the reply for a call that never ran.
    /// The last element is the approval actor when the press is not the approval
    /// (219 §2.2: an always-ask call raises a real card).
    #[allow(clippy::type_complexity)]
    fn admit_app(
        &mut self,
        call: &ToolCall,
        activation: Option<&UserActivation>,
        expected_identity: &str,
    ) -> Result<
        (
            DynTool,
            ToolSpec,
            Option<Admitted>,
            Option<ActorRef<Approval>>,
        ),
        Refused,
    > {
        use Refused::Unavailable;
        let name = &call.name;
        let tool = self
            .visible(call.session, name, false)
            .ok_or_else(|| Unavailable(format!("unknown tool: {name}")))?;
        // Availability before permission: a gated tool that is off does not
        // exist for this session, so no activation is ever asked for.
        let admitted = self
            .admit(call.session, name)
            .map_err(|why| Unavailable(format!("{name}: {}: {why}", communication::DISABLED)))?;
        let mut spec = tool.spec();
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
        // The identity above binds the declared spec; the gate sees the call's risk.
        spec.risk = call_risk(&tool, call.session, spec.risk, &call.args);
        let protected = self.protected_write(call.session, name, spec.risk, &call.args);
        let mut card = None;
        match self.effective(Some(call.session), name, spec.risk, mentions, protected) {
            PolicyAction::Allow => {}
            PolicyAction::Deny => {
                let why = self.denied(call.session, name, spec.risk, protected);
                return Err(Unavailable(format!("{name}: {why}")));
            }
            PolicyAction::Ask => match activation {
                // 210 §7.2: an app press is not the owner's approval of a human-only call.
                _ if tool.human_only() => {
                    return Err(Unavailable(format!(
                        "{name}: only the owner can approve this tool, in theuth itself; an app cannot run it"
                    )));
                }
                None => return Err(Refused::NeedsApproval),
                // 219 §2.2: nor is it the approval of an always-ask call: the press raises a card.
                Some(_) if self.always(Some(call.session), name, spec.risk, protected) => {
                    card = Some(
                        self.approval_actor()
                            .map_err(|why| Unavailable(format!("{name}: {why}")))?,
                    );
                }
                Some(a) => tracing::info!(
                    tool = %name, risk = ?spec.risk, secrets = mentions,
                    record = %a.record_call_id, action = %a.action_id, request = %a.request_id,
                    "app call approved by the user's activation"
                ),
            },
        }
        Ok((tool, spec, admitted, card))
    }

    /// Admission, approval (if `card`), secret expansion and the tool
    /// body, off the mailbox; `done` receives the output when the body has
    /// actually stopped. Shared by [`Invoke`] and [`AppInvoke`]: the app
    /// entry arrives here with `card == None`, its `Ask` already answered by
    /// the activation in [`Self::admit_app`], except for an always-ask call
    /// (219 §2.2). `admitted`
    /// is the communication admission from [`Self::admit`], re-checked in
    /// [`invoke`] once approval resolved (50 §8). `promote_after` is the
    /// session's promotion delay (145 §0.4) — [`Invoke`] only; an app call
    /// is never promoted.
    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &mut self,
        tool: DynTool,
        spec: ToolSpec,
        call: ToolCall,
        card: Option<Card>,
        admitted: Option<Admitted>,
        promote_after: Option<Duration>,
        done: impl FnOnce(ToolOutput) + Send + 'static,
    ) {
        // Everything that awaits leaves the mailbox: admission (so `Agents`
        // can still ask this actor while deciding), approval, the tool body.
        // The `Activity` lives in this task until the output is handed back:
        // a caller dropping the ask or synthesizing a cancelled response does
        // not retire the entry; only this task finishing does (47 §4.2 rule 2).
        let sink = self.activity.clone();
        let gate = self.gate.clone();
        let journal = self.journal.clone();
        let resources = self.resources_for(call.session);
        let inflight = self.inflight.clone();
        // 16 §5 (2026-11-11): only an owner's call may reach the local network.
        let lan = self.lan_allowed(call.session);
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
                    tool,
                    spec,
                    call,
                    card,
                    admitted,
                    activity,
                    gate,
                    resources,
                    journal,
                    promote_after,
                    inflight,
                    lan,
                ))
                .await;
            drop(running);
            done(out);
            // The map entry goes when this task ends; the last clone settles.
            drop(held);
        });
    }
}

/// An approval card to raise before a call runs (219 §2.3: `always_ask` marks it so).
struct Card {
    approval: ActorRef<Approval>,
    always_ask: bool,
}

impl Message<AppInvoke> for ToolRegistry {
    type Reply = DelegatedReply<AppInvoked>;

    async fn handle(&mut self, m: AppInvoke, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.admit_app(&m.call, m.activation.as_ref(), &m.expected_identity) {
            Err(r) => tx.send(r.into()),
            Ok((tool, spec, admitted, approval)) => {
                let card = approval.map(|approval| Card {
                    approval,
                    always_ask: true,
                });
                self.dispatch(tool, spec, m.call, card, admitted, None, move |out| {
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
            .visible(m.session, &m.name, false)
            .ok_or_else(|| format!("unknown tool: {}", m.name))?;
        let mentions = m.args.as_ref().is_some_and(|a| mentions(&tool, a));
        let risk = m
            .args
            .as_ref()
            .map(|a| call_risk(&tool, m.session, tool.spec().risk, a));
        let protected = m.args.as_ref().is_some_and(|a| {
            self.protected_write(m.session, &m.name, risk.unwrap_or(Risk::ReadOnly), a)
        });
        Ok(self.inspect(&tool, Some(m.session), mentions, risk, protected))
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
            .map(|(_, t)| self.inspect(t, session, false, None, false))
            .filter(|i| session.is_none_or(|s| self.capped(s, &i.spec.name, i.spec.risk).is_none()))
            .collect();
        out.sort_by(|a, b| a.spec.name.cmp(&b.spec.name));
        out
    }
}

impl Message<ExternalApproval> for ToolRegistry {
    type Reply = DelegatedReply<ExternalVerdict>;

    async fn handle(
        &mut self,
        m: ExternalApproval,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let approval = match self.gate(m.session, &m.tool, m.risk, false, false) {
            Ok(None) => {
                tx.send(ExternalVerdict::plain(Decision::Allow));
                return reply;
            }
            Ok(Some(a)) => a,
            Err(why) => {
                tracing::info!(tool = %m.tool, "external approval: {why}");
                tx.send(ExternalVerdict::plain(Decision::Deny));
                return reply;
            }
        };
        let human_only = self.human_only(&m.tool);
        let always_ask = self.always(Some(m.session), &m.tool, m.risk, false);
        tokio::spawn(async move {
            let req = RequestApproval {
                session: m.session,
                tool: m.tool,
                args: m.args,
                risk: m.risk,
                call_id: None,
                always_ask,
            };
            let asked = if human_only {
                approval.ask(RequestHumanOnly(req)).await.ok()
            } else {
                approval.ask(req).await.ok()
            };
            // Timeout already resolves as `Deny`; an unreachable actor is one too.
            // 207: a dismissal is carried, not flattened into a deny.
            tx.send(asked.map_or(ExternalVerdict::plain(Decision::Deny), |r| {
                ExternalVerdict {
                    decision: r.decision,
                    dismissed: r.dismissed,
                }
            }));
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
            .filter(|spec| session.is_none_or(|s| self.capped(s, &spec.name, spec.risk).is_none()))
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
        let mut names: Vec<String> = set.iter().cloned().collect();
        names.sort();
        if names.is_empty() {
            self.hidden.remove(&m.session);
        }
        self.hidden_epoch += 1;
        self.journal.hidden(m.session, names).await;
    }
}

impl Message<HiddenEpoch> for ToolRegistry {
    type Reply = u64;

    async fn handle(&mut self, _: HiddenEpoch, _: &mut Context<Self, Self::Reply>) -> u64 {
        self.hidden_epoch
    }
}

impl Message<RestoreHidden> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: RestoreHidden, _: &mut Context<Self, Self::Reply>) {
        if m.tools.is_empty() {
            self.hidden.remove(&m.session);
        } else {
            self.hidden.insert(m.session, m.tools.into_iter().collect());
        }
        self.hidden_epoch += 1;
    }
}

impl Message<ListBuiltins> for ToolRegistry {
    type Reply = Vec<BuiltinCategory>;

    async fn handle(
        &mut self,
        ListBuiltins(session): ListBuiltins,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<BuiltinCategory> {
        let chain = session.map(|s| self.hidden_chain(s)).unwrap_or_default();
        let mut by_category: HashMap<&'static str, Vec<BuiltinTool>> = HashMap::new();
        for (name, tool) in &self.tools {
            if !matches!(tool.source(), ToolSource::Native { .. }) {
                continue;
            }
            let category = builtins::category_of(name);
            let key = builtins::category_key(category);
            let tool_category = if chain.is_empty() {
                None
            } else {
                self.effective_category_of(name)
                    .map(|id| categories::tool_category_key(&id))
            };
            let names = |set: &HashSet<String>| set.contains(name) || set.contains(&key);
            let spec = tool.spec();
            by_category.entry(category).or_default().push(BuiltinTool {
                name: name.clone(),
                description: builtins::summary(&spec.description),
                risk: spec.risk,
                direct: spec.direct,
                disabled: names(&self.builtins_disabled),
                hidden: chain.iter().any(|set| {
                    names(set) || tool_category.as_ref().is_some_and(|key| set.contains(key))
                }),
            });
        }
        CATEGORIES
            .iter()
            .filter_map(|c| {
                let mut tools = by_category.remove(c.id)?;
                tools.sort_by(|a, b| a.name.cmp(&b.name));
                Some(BuiltinCategory {
                    id: c.id.to_owned(),
                    label: c.label.to_owned(),
                    description: c.description.to_owned(),
                    tools,
                })
            })
            .collect()
    }
}

impl Message<SetBuiltinsDisabled> for ToolRegistry {
    type Reply = Result<(), BuiltinsSwitchError>;

    async fn handle(
        &mut self,
        m: SetBuiltinsDisabled,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), BuiltinsSwitchError> {
        // Removing also takes a stale entry (its tool no longer registered); adding stays strict.
        let unknown: Vec<String> = m
            .names
            .iter()
            .filter(|n| {
                !self.is_builtin_name(n) && (m.disabled || !self.builtins_disabled.contains(*n))
            })
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(BuiltinsSwitchError::Unknown(unknown));
        }
        let mut next = self.builtins_disabled.clone();
        if m.disabled {
            next.extend(m.names);
        } else {
            next.retain(|n| !m.names.contains(n));
        }
        if next == self.builtins_disabled {
            return Ok(());
        }
        let mut sorted: Vec<&String> = next.iter().collect();
        sorted.sort();
        // JSON is YAML flow syntax, and quoting keeps `cat:<id>` a plain string.
        let value = serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".to_owned());
        // Persist first: a switch that cannot be saved is not applied, so it never
        // silently reverts at the next start.
        runtime::write_nested_config_value(
            &self.config_path,
            "tools",
            "builtins",
            "disabled",
            &value,
        )
        .map_err(|e| {
            BuiltinsSwitchError::NotWritten(format!("{}: {e}", self.config_path.display()))
        })?;
        self.builtins_disabled = next;
        // Running turns re-list before their next model call; python re-lists per cell.
        self.hidden_epoch += 1;
        Ok(())
    }
}

impl Message<ListToolCategories> for ToolRegistry {
    type Reply = Vec<ToolCategoryInfo>;

    async fn handle(
        &mut self,
        ListToolCategories(session): ListToolCategories,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<ToolCategoryInfo> {
        let chain = session.map(|s| self.hidden_chain(s)).unwrap_or_default();
        let (mut native, mut mcp) = self.category_members();
        let mut infos =
            Vec::with_capacity(categories::DEFAULT_CATEGORIES.len() + self.categories.custom.len());
        for c in categories::DEFAULT_CATEGORIES {
            infos.push(self.category_info(
                (c.id, c.label, c.description),
                false,
                &chain,
                &mut native,
                &mut mcp,
            ));
        }
        let mut custom: Vec<_> = self.categories.custom.iter().collect();
        custom.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
        for c in custom {
            infos.push(self.category_info(
                (&c.id, &c.label, &c.description),
                true,
                &chain,
                &mut native,
                &mut mcp,
            ));
        }
        infos
    }
}

impl Message<DefaultHiddenCategories> for ToolRegistry {
    type Reply = Vec<String>;

    async fn handle(
        &mut self,
        _: DefaultHiddenCategories,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<String> {
        self.categories
            .default_disabled
            .iter()
            .map(|id| categories::tool_category_key(id))
            .collect()
    }
}

impl Message<UpsertToolCategory> for ToolRegistry {
    type Reply = Result<ToolCategoryInfo, String>;

    async fn handle(
        &mut self,
        m: UpsertToolCategory,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<ToolCategoryInfo, String> {
        let label = m.label.trim().to_owned();
        if label.is_empty() {
            return Err("category label cannot be empty".to_owned());
        }
        let description = m.description.unwrap_or_default();
        let mut next = self.categories.custom.clone();
        let definition = match m.id {
            Some(id) => {
                if categories::default_category(&id).is_some() {
                    return Err(format!("default category `{id}` cannot be changed"));
                }
                let definition = CustomCategoryDef {
                    id,
                    label,
                    description,
                };
                if let Some(current) = next.iter_mut().find(|c| c.id == definition.id) {
                    *current = definition.clone();
                } else {
                    categories::validate_category_id(&definition.id)?;
                    next.push(definition.clone());
                }
                definition
            }
            None => {
                let base = categories::slugify_category_label(&label);
                categories::validate_category_id(&base)?;
                let mut id = base.clone();
                let mut suffix = 2_u64;
                while self.category_exists(&id) {
                    let ending = format!("-{suffix}");
                    let stem_len = 48_usize.saturating_sub(ending.len()).min(base.len());
                    id = format!("{}{ending}", &base[..stem_len]);
                    categories::validate_category_id(&id)?;
                    suffix += 1;
                }
                let definition = CustomCategoryDef {
                    id,
                    label,
                    description,
                };
                next.push(definition.clone());
                definition
            }
        };

        self.persist_categories("custom", &next)?;
        self.categories.custom = next;
        let (mut native, mut mcp) = self.category_members();
        Ok(self.category_info(
            (&definition.id, &definition.label, &definition.description),
            true,
            &[],
            &mut native,
            &mut mcp,
        ))
    }
}

impl Message<DeleteToolCategory> for ToolRegistry {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: DeleteToolCategory,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        if categories::default_category(&m.id).is_some() {
            return Err(format!("default category `{}` cannot be deleted", m.id));
        }
        if !self.categories.custom.iter().any(|c| c.id == m.id) {
            return Err(format!("unknown custom category `{}`", m.id));
        }

        let mut next = self.categories.clone();
        next.custom.retain(|c| c.id != m.id);
        next.mcp_assignments.retain(|_, category| category != &m.id);
        next.tool_assignments
            .retain(|_, category| category != &m.id);
        next.default_disabled.retain(|id| id != &m.id);

        self.persist_categories("custom", &next.custom)?;
        if next.mcp_assignments != self.categories.mcp_assignments {
            self.persist_categories("mcp_assignments", &next.mcp_assignments)?;
        }
        if next.tool_assignments != self.categories.tool_assignments {
            self.persist_categories("tool_assignments", &next.tool_assignments)?;
        }
        if next.default_disabled != self.categories.default_disabled {
            self.persist_categories("default_disabled", &next.default_disabled)?;
        }
        self.categories = next;
        self.hidden_epoch += 1;
        Ok(())
    }
}

impl Message<SetMcpCategory> for ToolRegistry {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetMcpCategory,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        if let Some(category) = &m.category
            && !self.category_exists(category)
        {
            return Err(format!("unknown tool category `{category}`"));
        }
        let mut next = self.categories.mcp_assignments.clone();
        match &m.category {
            Some(category) => {
                next.insert(m.server, category.clone());
            }
            None => {
                next.remove(&m.server);
            }
        }
        if next == self.categories.mcp_assignments {
            return Ok(());
        }
        self.persist_categories("mcp_assignments", &next)?;
        self.categories.mcp_assignments = next;
        self.hidden_epoch += 1;
        Ok(())
    }
}

impl Message<SetToolCategory> for ToolRegistry {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetToolCategory,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        if m.category.is_some() && !self.is_native(&m.tool) {
            return Err(format!("`{}` is not a registered native tool", m.tool));
        }
        if let Some(category) = &m.category
            && !self.category_exists(category)
        {
            return Err(format!("unknown tool category `{category}`"));
        }
        let mut next = self.categories.tool_assignments.clone();
        match m.category.as_deref() {
            Some(category) if category != categories::default_category_for_builtin(&m.tool) => {
                next.insert(m.tool, category.to_owned());
            }
            _ => {
                next.remove(&m.tool);
            }
        }
        if next == self.categories.tool_assignments {
            return Ok(());
        }
        self.persist_categories("tool_assignments", &next)?;
        self.categories.tool_assignments = next;
        self.hidden_epoch += 1;
        Ok(())
    }
}

impl Message<SetDefaultToolCategories> for ToolRegistry {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        mut m: SetDefaultToolCategories,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        for id in &m.disabled {
            if !self.category_exists(id) {
                return Err(format!("unknown tool category `{id}`"));
            }
        }
        m.disabled.sort();
        m.disabled.dedup();
        if m.disabled == self.categories.default_disabled {
            return Ok(());
        }
        self.persist_categories("default_disabled", &m.disabled)?;
        self.categories.default_disabled = m.disabled;
        Ok(())
    }
}

impl Message<SetAlwaysAsk> for ToolRegistry {
    type Reply = Result<AlwaysAskState, AlwaysAskError>;

    async fn handle(
        &mut self,
        m: SetAlwaysAsk,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<AlwaysAskState, AlwaysAskError> {
        let tool = m.tool.trim();
        if tool.is_empty() {
            return Err(AlwaysAskError::Empty);
        }
        // Removing a loop-local entry (hand-edited into config.yaml) stays possible.
        if m.on && LOOP_LOCAL_TOOLS.contains(&tool) {
            return Err(AlwaysAskError::LoopLocal(tool.to_owned()));
        }
        match m.session {
            None => {
                if self.always_ask.contains(tool) == m.on {
                    return Ok(self.always_ask_state(None));
                }
                let mut next = self.always_ask.clone();
                if m.on {
                    next.insert(tool.to_owned());
                } else {
                    next.remove(tool);
                }
                let value =
                    serde_json::to_string(&sorted(&next)).unwrap_or_else(|_| "[]".to_owned());
                // Persist first (the `SetBuiltinsDisabled` pattern): an entry that cannot be
                // saved is not applied, so it never silently changes at the next start.
                runtime::write_config_value(&self.config_path, "tools", "always_ask", &value)
                    .map_err(|e| {
                        AlwaysAskError::NotWritten(format!(
                            "config.yaml not written: {}: {e}",
                            self.config_path.display()
                        ))
                    })?;
                self.always_ask = next;
            }
            Some(id) => {
                // The stored list is unknown: writing ours over it would drop its entries at
                // the next spawn, so nothing is written until a restore reads it.
                if self.always_ask_unreadable.contains(&id) {
                    return Err(AlwaysAskError::NotWritten(
                        "sessions.always_ask not written: the stored list could not be read (see the log)".to_owned(),
                    ));
                }
                let mut next = self
                    .always_ask_session
                    .get(&id)
                    .cloned()
                    .unwrap_or_default();
                let changed = if m.on {
                    next.insert(tool.to_owned())
                } else {
                    next.remove(tool)
                };
                if !changed {
                    return Ok(self.always_ask_state(m.session));
                }
                // Write first, apply second (as the harness-wide path): an entry the store
                // did not keep is not applied, so it never silently changes at the next spawn.
                self.journal
                    .always_ask(id, sorted(&next))
                    .await
                    .map_err(|e| {
                        AlwaysAskError::NotWritten(format!("sessions.always_ask not written: {e}"))
                    })?;
                if next.is_empty() {
                    self.always_ask_session.remove(&id);
                } else {
                    self.always_ask_session.insert(id, next);
                }
            }
        }
        Ok(self.always_ask_state(m.session))
    }
}

impl Message<RestoreAlwaysAsk> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: RestoreAlwaysAsk, _: &mut Context<Self, Self::Reply>) {
        self.always_ask_unreadable.remove(&m.session);
        if m.names.is_empty() {
            self.always_ask_session.remove(&m.session);
        } else {
            self.always_ask_session
                .insert(m.session, m.names.into_iter().collect());
        }
    }
}

impl Message<AlwaysAskUnreadable> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: AlwaysAskUnreadable, _: &mut Context<Self, Self::Reply>) {
        self.always_ask_unreadable.insert(m.session);
    }
}

impl Message<GetAlwaysAsk> for ToolRegistry {
    type Reply = AlwaysAskState;

    async fn handle(
        &mut self,
        m: GetAlwaysAsk,
        _: &mut Context<Self, Self::Reply>,
    ) -> AlwaysAskState {
        self.always_ask_state(m.session)
    }
}

/// A set's entries, sorted.
fn sorted(set: &HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = set.iter().cloned().collect();
    v.sort();
    v
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

/// `ask` → `Option<Risk>` (183 Z3-R2): the tightest ceiling on `session` right now — its own
/// or an ancestor's [`SetCap`], or a contact turn's [`SetTurnCap`]. A child spawned now gets
/// it as its own cap, so it never outlives the turn uncapped.
#[derive(Clone, Copy, Debug)]
pub struct EffectiveCap(pub SessionId);

impl Message<EffectiveCap> for ToolRegistry {
    type Reply = Option<Risk>;

    async fn handle(
        &mut self,
        m: EffectiveCap,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<Risk> {
        self.cap_for(m.0)
    }
}

/// `ask` → `bool` (183 Z3-R3): `session` is a contact session (an inbox or under one, or a
/// contact's turn runs in it): its prompt carries nothing about the owner's machine.
#[derive(Clone, Copy, Debug)]
pub struct ContactScoped(pub SessionId);

impl Message<ContactScoped> for ToolRegistry {
    type Reply = bool;

    async fn handle(&mut self, m: ContactScoped, _: &mut Context<Self, Self::Reply>) -> bool {
        self.contact_scoped(m.0)
    }
}

/// `ask` (16 §5, 2026-11-11): tool calls of `session` run without the local network (`on`), or
/// with it again. For a plugin that calls a tool on behalf of a caller whose grant it carries
/// (a notebook source's ingest job, `document::ingest`).
#[derive(Clone, Copy, Debug)]
pub struct SetNoLan {
    /// The session.
    pub session: SessionId,
    /// Set or lift.
    pub on: bool,
}

impl Message<SetNoLan> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetNoLan, _: &mut Context<Self, Self::Reply>) {
        if m.on {
            self.no_lan.insert(m.session);
        } else {
            self.no_lan.remove(&m.session);
        }
    }
}

/// `tell` (210 §7.2): `session` is bound to a chat thread whose user is not the owner (`on`),
/// or no longer is. Sent by the Slack and Telegram channels on every bind and for every bound
/// thread at start; a human-only tool is refused in such a session and its subagents.
#[derive(Clone, Copy, Debug)]
pub struct SetNonOwnerThread {
    /// The bound session.
    pub session: SessionId,
    /// Set or lift.
    pub on: bool,
}

impl Message<SetNonOwnerThread> for ToolRegistry {
    type Reply = ();

    async fn handle(&mut self, m: SetNonOwnerThread, _: &mut Context<Self, Self::Reply>) {
        if m.on {
            self.non_owner_thread.insert(m.session);
        } else {
            self.non_owner_thread.remove(&m.session);
        }
    }
}

/// [`SetNonOwnerThread`] to the registered registry, without waiting; a no-op when none runs.
/// Synchronous so a channel can call it under its own lock.
pub fn mark_non_owner_thread(session: SessionId, on: bool) {
    if let Ok(Some(registry)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) {
        let _ = registry.tell(SetNonOwnerThread { session, on }).try_send();
    }
}

/// `ask` → `Vec<String>` (183 Z3-R2): the names of `.1` a contact session's allowlist admits,
/// so the loop's own tools (`python`, `todo`, …) obey it like the registry's. No allowlist on
/// the session admits them all.
#[derive(Clone, Debug)]
pub struct Allowlisted(pub SessionId, pub Vec<String>);

impl Message<Allowlisted> for ToolRegistry {
    type Reply = Vec<String>;

    async fn handle(
        &mut self,
        Allowlisted(id, names): Allowlisted,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<String> {
        names
            .into_iter()
            .filter(|n| self.allowlist_admits(id, n))
            .collect()
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

impl Message<ToolSourceOf> for ToolRegistry {
    type Reply = Option<ToolSource>;

    async fn handle(
        &mut self,
        m: ToolSourceOf,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<ToolSource> {
        self.tools.get(&m.0).map(|t| t.source())
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

impl Message<InstallJournal> for ToolRegistry {
    type Reply = ();

    /// 109 §3.3 step 6: keep the journal for [`invoke`] and hand the same
    /// handle to the jobs, approval and ask actors (none of them asks back
    /// into this actor, so awaiting them cannot deadlock). An actor that is
    /// not running simply keeps its `NoopJournal`.
    async fn handle(&mut self, m: InstallJournal, _: &mut Context<Self, Self::Reply>) {
        self.journal = m.0.clone();
        *lock(&JOURNAL) = m.0.clone();
        if let Ok(Some(jobs)) = ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
            let _ = jobs.ask(InstallJournal(m.0.clone())).await;
        }
        if let Ok(Some(approval)) = ActorRef::<Approval>::lookup(APPROVAL_ACTOR) {
            let _ = approval.ask(InstallJournal(m.0.clone())).await;
        }
        if let Ok(Some(ask)) = ActorRef::<AskUser>::lookup(ASK_ACTOR) {
            let _ = ask.ask(InstallJournal(m.0)).await;
        }
        tracing::info!("109: journal installed");
    }
}

/// 109 §6.1: the installed journal, for the human-wait tools other crates
/// register (`desktop_handoff`, `secret_request*`, `send_message`), which
/// receive no [`InstallJournal`] of their own. [`NoopJournal`] before the
/// install.
static JOURNAL: std::sync::LazyLock<Mutex<Arc<dyn Journal>>> =
    std::sync::LazyLock::new(|| Mutex::new(Arc::new(crate::NoopJournal)));

/// The journal [`InstallJournal`] installed last ([`NoopJournal`] until then).
pub fn journal() -> Arc<dyn Journal> {
    lock(&JOURNAL).clone()
}

/// 109 §6.2 (`Republish`): the pending `questions` row of a human-wait call
/// the resume path is about to re-invoke, by `(session, call_id)`. The tool
/// body takes it with [`take_republish`] and re-arms that question — same id,
/// remaining deadline — instead of raising a new one.
static REPUBLISH: std::sync::LazyLock<Mutex<HashMap<(SessionId, String), QuestionRow>>> =
    std::sync::LazyLock::new(Default::default);

/// Hand the row of `(row.session, row.tool_call_id)` to its next invocation.
pub fn republish(row: QuestionRow) {
    if let Some(call) = row.tool_call_id.clone() {
        lock(&REPUBLISH).insert((row.session, call), row);
    }
}

/// The row [`republish`] left for this call, if any; taken once.
pub fn take_republish(session: SessionId, call_id: &str) -> Option<QuestionRow> {
    lock(&REPUBLISH).remove(&(session, call_id.to_owned()))
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
        self.invoke(call, false, None, ctx)
    }
}

impl Message<HarnessInvoke> for ToolRegistry {
    type Reply = DelegatedReply<ToolOutput>;

    async fn handle(
        &mut self,
        HarnessInvoke(call): HarnessInvoke,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.invoke(call, true, None, ctx)
    }
}

impl Message<InvokeHumanOnly> for ToolRegistry {
    type Reply = DelegatedReply<ToolOutput>;

    async fn handle(
        &mut self,
        InvokeHumanOnly { call, requester }: InvokeHumanOnly,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.invoke(call, false, Some(requester), ctx)
    }
}

impl ToolRegistry {
    /// [`Invoke`] and, with `harness`, [`HarnessInvoke`] (206: the builtin switch skipped);
    /// with `requester`, [`InvokeHumanOnly`] (the policy is the tool's, its card human-only).
    fn invoke(
        &mut self,
        mut call: ToolCall,
        harness: bool,
        requester: Option<String>,
        ctx: &mut Context<Self, DelegatedReply<ToolOutput>>,
    ) -> DelegatedReply<ToolOutput> {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let Some(tool) = self.visible(call.session, &call.name, harness) else {
            // 206: a builtin the user switched off says so; anything else stays unknown.
            let text = if harness
                || self.mcp_hidden(call.session, &call.name)
                || !self.allowlist_admits(call.session, &call.name)
                || !self.builtin_switched(call.session, &call.name)
            {
                format!("unknown tool: {}", call.name)
            } else {
                format!("{} is turned off (Settings > Tools > Builtins)", call.name)
            };
            tx.send(ToolOutput::err(text));
            return reply;
        };
        if self.contact_scoped(call.session) {
            bound_for_contact(&call.name, &mut call.args);
        }
        let mut spec = tool.spec();
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
        // 209 D14: the approval card and the gate both see the call's risk.
        spec.risk = call_risk(&tool, call.session, spec.risk, &call.args);
        let protected = self.protected_write(call.session, &call.name, spec.risk, &call.args);
        let card = match self.gate(call.session, &call.name, spec.risk, mentions, protected) {
            Ok(a) => a.map(|approval| Card {
                approval,
                always_ask: self.always(Some(call.session), &call.name, spec.risk, protected),
            }),
            Err(why) => {
                tx.send(ToolOutput::err(format!("{}: {why}", call.name)));
                return reply;
            }
        };
        let tool: DynTool = match requester {
            Some(requester) => Arc::new(Brokered { inner: tool, requester }),
            None => tool,
        };
        let promote_after = self.promote_after.get(&call.session).copied();
        self.dispatch(
            tool,
            spec,
            call,
            card,
            admitted,
            promote_after,
            move |out| tx.send(out),
        );
        reply
    }
}

/// [`InvokeHumanOnly`]'s view of a tool, applied after the gate: the same tool, but its approval
/// card is human-only and names the requester.
struct Brokered {
    inner: DynTool,
    requester: String,
}

impl crate::ErasedTool for Brokered {
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }
    fn source(&self) -> ToolSource {
        self.inner.source()
    }
    fn origin(&self) -> String {
        self.inner.origin()
    }
    fn consumes_secrets(&self) -> bool {
        self.inner.consumes_secrets()
    }
    fn promotable(&self) -> bool {
        self.inner.promotable()
    }
    fn credentialed(&self, session: SessionId, args: &serde_json::Value) -> bool {
        self.inner.credentialed(session, args)
    }
    fn approved_by_card(&self) -> bool {
        self.inner.approved_by_card()
    }
    fn human_only(&self) -> bool {
        true
    }
    fn approval_preview(
        &self,
        args: &serde_json::Value,
        cx: PreviewCx,
    ) -> futures::future::BoxFuture<'_, anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>>>
    {
        let inner = self.inner.approval_preview(args, cx);
        let line = format!("{} wants to run {}", self.requester, self.inner.spec().name);
        Box::pin(async move {
            let mut extra = inner.await?.unwrap_or_default();
            extra.insert("requested_by".into(), line.into());
            Ok(Some(extra))
        })
    }
    fn call(
        &self,
        args: serde_json::Value,
        cx: ToolCx,
    ) -> futures::future::BoxFuture<'_, anyhow::Result<ToolOutput>> {
        self.inner.call(args, cx)
    }
}

/// 183 Z4-R1: per-call bounds on the web tools in a contact session, so one contact message
/// cannot spend the owner's crawl or search quota: `(tool, argument, max)`. Set where absent
/// too (the owner's defaults may be larger); a value that is not a whole number takes the max.
const CONTACT_WEB_BOUNDS: &[(&str, &str, u64)] = &[
    ("crawl", "limit", 20),
    ("crawl", "max_depth", 2),
    ("map_site", "limit", 200),
    ("search_web", "limit", 8),
    ("search_web", "read", 5),
];

fn bound_for_contact(name: &str, args: &mut serde_json::Value) {
    let Some(obj) = args.as_object_mut() else {
        return;
    };
    for (_, key, max) in CONTACT_WEB_BOUNDS.iter().filter(|(t, ..)| *t == name) {
        let v = obj
            .get(*key)
            .and_then(serde_json::Value::as_u64)
            .map_or(*max, |v| v.min(*max));
        obj.insert((*key).into(), v.into());
    }
}

/// The gate's secret input: a placeholder in `args` counts only for a tool
/// that consumes it ([`crate::ErasedTool::consumes_secrets`]); for one that
/// stores its arguments as data the placeholder is the data.
fn mentions(tool: &DynTool, args: &serde_json::Value) -> bool {
    tool.consumes_secrets() && crate::secrets::mentions(args)
}

/// The risk one call is gated at (209 D14, §9): a credentialed call
/// ([`crate::ErasedTool::credentialed`]) is at least `Mutating`, so the cap
/// check and the policy both see it. Only asked of tools declared below
/// `Mutating`, where the answer can change anything.
fn call_risk(tool: &DynTool, session: SessionId, risk: Risk, args: &serde_json::Value) -> Risk {
    if risk < Risk::Mutating && tool.credentialed(session, args) {
        Risk::Mutating
    } else {
        risk
    }
}

/// The binding an app holds a tool to: sha256 hex over the canonical JSON
/// of `{name, schema, origin}` ([`crate::ErasedTool::origin`]). Private so
/// nothing outside this actor can recompute it from display data. Object
/// keys are sorted at every depth before hashing (arrays keep their order,
/// `required`/`enum` are positional): the unified feature graph turns on
/// `serde_json/preserve_order`, so an MCP server re-sending the same schema
/// with keys in a different order must still hash equal.
fn identity(tool: &DynTool, spec: &ToolSpec) -> String {
    let mut canon = serde_json::json!({
        "name": spec.name,
        "schema": spec.schema,
        "origin": tool.origin(),
    });
    canon.sort_all_objects();
    platform::sha256::hex(canon.to_string().as_bytes())
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
/// the body runs before the suspension resumed. `promote_after` (145 §0.4):
/// a promotable body still running after it is handed to `Jobs` as a
/// promoted job and the call returns a `{job}` stub. `inflight` holds the
/// call's token while the body runs inline ([`CancelCalls`]).
// Every argument is a distinct per-call input; a bundling struct would only be built at the one call site.
#[allow(clippy::too_many_arguments)]
async fn invoke(
    tool: DynTool,
    spec: ToolSpec,
    mut call: ToolCall,
    card: Option<Card>,
    admitted: Option<Admitted>,
    activity: Activity,
    gate: Option<Arc<dyn ExecutionGate>>,
    resources: Arc<SessionResources>,
    journal: Arc<dyn Journal>,
    promote_after: Option<Duration>,
    inflight: Arc<Mutex<HashMap<(SessionId, String), CancellationToken>>>,
    lan: bool,
) -> ToolOutput {
    let session = call.session;
    let call_id = call.id.clone();
    let suspend = || gate.as_ref().and_then(|g| g.suspend(session));
    let mut approved = None;
    if let Some(Card {
        approval,
        always_ask,
    }) = card
    {
        // 210 §7.3: the tool's own card details (`share_publish`: the exact URL) join the args
        // the card shows; `call` keeps the model's args. A refusal here raises no card.
        let mut args = call.args.clone();
        let preview = PreviewCx {
            session,
            call_id: call_id.clone(),
        };
        match tool.approval_preview(&call.args, preview).await {
            Ok(Some(extra)) => {
                if let Some(o) = args.as_object_mut() {
                    o.extend(extra);
                }
            }
            Ok(None) => {}
            Err(e) => return ToolOutput::err(format!("{}: {e:#}", spec.name)),
        }
        let req = RequestApproval {
            session: call.session,
            tool: spec.name.clone(),
            args,
            risk: spec.risk,
            call_id: Some(call_id.clone()),
            always_ask,
        };
        let suspended = suspend();
        let human_only = tool.human_only();
        let answer = if human_only {
            approval
                .ask(RequestHumanOnly(req))
                .await
                .map_err(|e| e.to_string())
        } else {
            approval.ask(req).await.map_err(|e| e.to_string())
        };
        match answer {
            Ok(r) if r.decision == Decision::Allow => {
                approved = Some(crate::approval::Approved {
                    id: r.id,
                    surface: if human_only { "owner" } else { "human" },
                });
            }
            // Denied / timed out / unavailable: the suspension drops
            // unresumed and nothing runs.
            // 207: left to the agent (Attention dismiss): not run.
            Ok(r) if r.dismissed => {
                return ToolOutput::err(format!(
                    "{}: not run — the user left this call to you without approving it. Do not retry it as is; find a way that needs no approval, or say in your reply why it is needed.",
                    spec.name
                ));
            }
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
    let cancel = CancellationToken::new();
    let promote_after = promote_after.filter(|_| tool.promotable());
    let cx = ToolCx {
        session: call.session,
        call_id: call.id,
        quiet: call.quiet,
        resources,
        cancel: cancel.clone(),
        promote_after,
    };
    // A tool whose whole body is a human wait executes nothing bounded; the
    // suspension ends with the body (the next call in the batch, or the
    // model, resumes under the caller's own accounting).
    let waiting = (spec.name == <AskUserTool as crate::Tool>::NAME)
        .then(suspend)
        .flatten();
    // 109 §3.3 step 6: the gate resolved and nothing can refuse the call any
    // more; from here its effect is unknown to a restart, so the journal says
    // `started` before the body runs.
    journal.call_started(session, &call_id).await;
    // Own task so a panicking tool is an error output, not a dead registry.
    // Task-locals do not cross `spawn`: re-enter the scopes on the spawned
    // future, so the body sees `current()` (a panic there marks the operation
    // unknown), a gated tool reads *this* admission at dispatch rather than
    // a fresh generation minted by an off→on since the check above (50 §8),
    // and the web tools read the session's `egress::lan()`.
    let body = activity.scope(communication::scope(
        admitted,
        crate::egress::scope(
            lan,
            crate::approval::approved_scope(
                approved,
                async move { tool.call(call.args, cx).await },
            ),
        ),
    ));
    let key = (session, call_id.clone());
    lock(&inflight).insert(key.clone(), cancel.clone());
    let mut handle = tokio::spawn(body);
    let name = spec.name;
    let finished = |r: Result<anyhow::Result<ToolOutput>, tokio::task::JoinError>| match r {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => ToolOutput::err(format!("{name}: {e:#}")),
        Err(e) => ToolOutput::err(format!("{name}: tool panicked: {e}")),
    };
    // Promotion (145 §0.4): a promotable body still running after the delay,
    // with a `Jobs` actor to adopt it. Its token leaves `inflight` first — a
    // turn cancel after this is not the job's business.
    let mut out = match promote_after {
        Some(after) => match tokio::time::timeout(after, &mut handle).await {
            Ok(r) => finished(r),
            Err(_) => match ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
                Ok(Some(jobs)) => {
                    lock(&inflight).remove(&key);
                    promote(
                        &jobs,
                        session,
                        call_id.clone(),
                        &name,
                        cancel,
                        handle,
                        after,
                    )
                    .await
                }
                _ => finished(handle.await),
            },
        },
        None => finished(handle.await),
    };
    lock(&inflight).remove(&key);
    if let Some(w) = waiting {
        // Nothing runs after this in the invocation; resuming only puts the
        // batch's accounting back (a cancelled turn makes it a no-op).
        w.resume().await;
    }
    // Every arm: an error string may echo the expanded args.
    crate::secrets::redact_output(&mut out);
    // 109 §3.3 step 7: the result row is per call, committed as it returns.
    journal.call_finished(session, &call_id, &out).await;
    out
}

/// 145 §0.4: hand a body that outlived `promote_after` to `Jobs` and answer
/// the call with the `{job}` stub. The job owns the body from here: dropping
/// its future (a `cancel_job`) aborts the task. An adoption the jobs actor
/// refused is an error output — the body was dropped with the message.
#[allow(clippy::too_many_arguments)]
async fn promote(
    jobs: &ActorRef<Jobs>,
    session: SessionId,
    call_id: String,
    tool: &str,
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<anyhow::Result<ToolOutput>>,
    after: Duration,
) -> ToolOutput {
    let after = u64::try_from(after.as_millis()).unwrap_or(u64::MAX);
    let work = Box::pin(async move {
        let _abort = AbortOnDrop(handle.abort_handle());
        handle
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("tool panicked: {e}")))
    });
    match jobs
        .ask(jobs::Adopt {
            session,
            call_id,
            tool: tool.to_owned(),
            cancel,
            work,
        })
        .await
    {
        Ok(id) => {
            let v = serde_json::json!({
                "job": id,
                "tool": tool,
                "promoted_after_ms": after,
                "note": "the result arrives later as <job_result>; do not wait or poll",
            });
            ToolOutput {
                content: v.to_string(),
                is_error: false,
                structured: Some(v),
                files: Vec::new(),
            }
        }
        Err(e) => ToolOutput::err(format!("{tool}: promotion failed: {e}")),
    }
}

/// Aborts the body's task when the adopted future is dropped unfinished.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
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
    let builtins: [DynTool; 20] = [
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
        Arc::new(jobs::CancelJobTool(jobs.clone())),
        Arc::new(reminder::RemindMeInTool {
            jobs: jobs.clone(),
            max_secs: cfg.reminder_max_secs,
        }),
        Arc::new(reminder::CancelReminderTool(jobs)),
        Arc::new(fs::ReadFile),
        Arc::new(fs::WriteFile),
        Arc::new(fs::ListDir),
        Arc::new(fs::Grep),
        Arc::new(glob::Glob),
        Arc::new(edit::Edit),
        Arc::new(crate::text::MoveText),
        Arc::new(crate::text::CopyText),
        Arc::new(crate::text::Indent),
        Arc::new(crate::text::ClipTool),
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
            pin: HashMap::new(),
            hidden: HashMap::new(),
            hidden_epoch: 0,
            builtins_disabled: cfg.builtins.disabled.into_iter().collect(),
            categories: cfg.categories,
            // Trimmed as `SetAlwaysAsk` stores them, so a hand-edited `" shell"` still matches.
            always_ask: cfg
                .always_ask
                .iter()
                .map(|n| n.trim())
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect(),
            always_ask_session: HashMap::new(),
            always_ask_unreadable: HashSet::new(),
            parents: HashMap::new(),
            cap: HashMap::new(),
            turn_cap: HashMap::new(),
            allow_only: HashMap::new(),
            turn_allow_only: HashMap::new(),
            no_lan: HashSet::new(),
            non_owner_thread: HashSet::new(),
            config_path: rt.data_dir.join("config.yaml"),
            resources: HashMap::new(),
            durable: SessionResources::durable_arc(&rt.data_dir),
            approval: None,
            activity: None,
            untracked: Arc::default(),
            communication: None,
            mcp_access: None,
            gate: None,
            journal: Arc::new(NoopJournal),
            participants: Vec::new(),
            expected: Vec::new(),
            shells,
            close_timeout: CLOSE_TIMEOUT,
            pty_idle,
            promote_after: HashMap::new(),
            inflight: Arc::default(),
        },
        runtime::subscriber_mailbox(),
    );
    registry.register(REGISTRY_ACTOR)?;
    Ok(registry)
}

#[cfg(test)]
pub(crate) mod tests {
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
            pin: HashMap::new(),
            hidden: HashMap::new(),
            hidden_epoch: 0,
            builtins_disabled: HashSet::new(),
            categories: ToolCategoriesConfig::default(),
            always_ask: HashSet::new(),
            always_ask_session: HashMap::new(),
            always_ask_unreadable: HashSet::new(),
            parents: HashMap::new(),
            cap: HashMap::new(),
            turn_cap: HashMap::new(),
            allow_only: HashMap::new(),
            turn_allow_only: HashMap::new(),
            no_lan: HashSet::new(),
            non_owner_thread: HashSet::new(),
            config_path: PathBuf::new(),
            resources: HashMap::new(),
            durable: SessionResources::durable_arc(std::path::Path::new("/tmp")),
            approval: None,
            activity,
            untracked: Arc::default(),
            communication: None,
            mcp_access: None,
            gate: None,
            journal: Arc::new(NoopJournal),
            participants: Vec::new(),
            expected: Vec::new(),
            shells: Arc::default(),
            close_timeout: Duration::from_millis(100),
            pty_idle: Arc::default(),
            promote_after: HashMap::new(),
            inflight: Arc::default(),
        }
    }

    pub(crate) fn registry(
        tools: Vec<DynTool>,
        activity: Option<ActivitySink>,
    ) -> ActorRef<ToolRegistry> {
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
    async fn tool_source_of_names_the_registrant() {
        let reg = registry(vec![Arc::new(Echo)], None);
        assert_eq!(
            reg.ask(ToolSourceOf("echo".into())).await.unwrap(),
            Some(ToolSource::native())
        );
        assert_eq!(reg.ask(ToolSourceOf("nope".into())).await.unwrap(), None);
        reg.ask(UnregisterTool("echo".into())).await.unwrap();
        assert_eq!(reg.ask(ToolSourceOf("echo".into())).await.unwrap(), None);
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

    /// Sleeps `ms` from its args unless its token fires first; `promotable`
    /// as configured (145 T1/T2/T4).
    struct Sleeper {
        promotable: bool,
        /// Bodies entered so far.
        entered: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Tool for Sleeper {
        const NAME: &'static str = "sleeper";
        const RISK: Risk = Risk::ReadOnly;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        fn promotable(&self) -> bool {
            self.promotable
        }
        async fn call(&self, args: Self::Args, cx: ToolCx) -> anyhow::Result<ToolOutput> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            let ms = args["ms"].as_u64().unwrap_or(0);
            tokio::select! {
                () = cx.cancel.cancelled() => Ok(ToolOutput::ok("cancelled")),
                () = tokio::time::sleep(Duration::from_millis(ms)) => Ok(ToolOutput::ok("slept")),
            }
        }
    }

    fn sleeper(promotable: bool) -> (DynTool, Arc<std::sync::atomic::AtomicUsize>) {
        let entered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Arc::new(Sleeper {
                promotable,
                entered: entered.clone(),
            }),
            entered,
        )
    }

    fn sleep_call(session: SessionId, ms: u64) -> ToolCall {
        ToolCall {
            args: serde_json::json!({ "ms": ms }),
            ..call("sleeper", session)
        }
    }

    /// 145 T1: a body still running after `promote_after` comes back at once
    /// as the `{job}` stub; the job then finishes with the tool's output. The
    /// one test that registers a `Jobs` actor under [`JOBS_ACTOR`].
    #[tokio::test]
    async fn slow_call_is_promoted_to_a_job() {
        let fx = jobs::tests::fixture(Duration::from_secs(1), Duration::from_secs(5)).await;
        fx.jobs.register(JOBS_ACTOR).unwrap();
        let (tool, _) = sleeper(true);
        let reg = registry(vec![tool], None);
        let session = fx.session;
        reg.tell(SetPromoteAfter {
            session,
            after: Some(Duration::from_millis(100)),
        })
        .await
        .unwrap();
        let started = std::time::Instant::now();
        let out = reg.ask(Invoke(sleep_call(session, 600))).await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(500), "{out:?}");
        assert!(!out.is_error, "{out:?}");
        let v = out.structured.expect("the stub is structured");
        assert_eq!(v["job"], "j1");
        assert_eq!(v["tool"], "sleeper");
        assert_eq!(v["promoted_after_ms"], 100);
        assert_eq!(
            v["note"],
            "the result arrives later as <job_result>; do not wait or poll"
        );
        assert_eq!(out.content, v.to_string());
        let (exit, output, inline) = jobs::tests::finished(&fx).await;
        assert_eq!(
            (exit, output.as_str(), inline),
            (jobs::JobExit::Exited(0), "slept", false)
        );
        // Promotion off again: the same call waits for its body.
        reg.tell(SetPromoteAfter {
            session,
            after: None,
        })
        .await
        .unwrap();
        let out = reg.ask(Invoke(sleep_call(session, 150))).await.unwrap();
        assert_eq!(out.content, "slept");
    }

    /// 145 T2: a `promotable() == false` tool is awaited to completion
    /// whatever `promote_after` says.
    #[tokio::test]
    async fn unpromotable_call_is_awaited() {
        let (tool, _) = sleeper(false);
        let reg = registry(vec![tool], None);
        let session = SessionId::new();
        reg.tell(SetPromoteAfter {
            session,
            after: Some(Duration::from_millis(20)),
        })
        .await
        .unwrap();
        let started = std::time::Instant::now();
        let out = reg.ask(Invoke(sleep_call(session, 250))).await.unwrap();
        assert_eq!(out.content, "slept");
        assert!(started.elapsed() >= Duration::from_millis(250));
    }

    /// 145 T4: `CancelCalls` fires the token of every in-flight call of the
    /// session — the body sees it and returns — and touches no other session.
    #[tokio::test]
    async fn cancel_calls_fires_inflight_tokens() {
        let (tool, entered) = sleeper(true);
        let reg = registry(vec![tool], None);
        let (session, other) = (SessionId::new(), SessionId::new());
        let mine = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ask(Invoke(sleep_call(session, 5_000))).await.unwrap() }
        });
        let theirs = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ask(Invoke(sleep_call(other, 300))).await.unwrap() }
        });
        while entered.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        reg.tell(CancelCalls { session }).await.unwrap();
        let out = tokio::time::timeout(Duration::from_secs(2), mine)
            .await
            .expect("the cancelled body returns at once")
            .unwrap();
        assert_eq!(out.content, "cancelled");
        assert_eq!(theirs.await.unwrap().content, "slept");
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

    /// An `mcp:<server>` hide covers every tool of that server for the session and its
    /// subagents — one the server registers later (a reconnect) too — and nothing else;
    /// each change moves [`HiddenEpoch`], and showing the server again restores its tools.
    #[tokio::test]
    async fn a_hidden_server_hides_its_tools_registered_now_or_later() {
        let reg = registry_with(
            vec![Arc::new(Echo), Arc::new(FilesMcp("search", Risk::ReadOnly))],
            None,
            Policy::default(),
            None,
        );
        let (parent, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        let names = |specs: Vec<ToolSpec>| specs.into_iter().map(|s| s.name).collect::<Vec<_>>();
        let before = reg.ask(HiddenEpoch).await.unwrap();

        reg.ask(SetHidden {
            session: parent,
            tools: vec![mcp_server_key("files")],
            hidden: true,
        })
        .await
        .unwrap();
        assert_ne!(reg.ask(HiddenEpoch).await.unwrap(), before);
        assert_eq!(
            names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo"]
        );
        // The server reconnects with a tool it did not offer before: hidden as well.
        reg.ask(RegisterTool(Arc::new(FilesMcp("delete", Risk::ReadOnly))))
            .await
            .unwrap();
        assert_eq!(
            names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo"]
        );
        assert!(
            reg.ask(Invoke(call("delete", child)))
                .await
                .unwrap()
                .is_error
        );
        assert_eq!(
            names(reg.ask(ListTools(None)).await.unwrap()),
            ["delete", "echo", "search"]
        );

        reg.ask(SetHidden {
            session: parent,
            tools: vec![mcp_server_key("files")],
            hidden: false,
        })
        .await
        .unwrap();
        assert_eq!(
            names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["delete", "echo", "search"]
        );
    }

    fn tool_names(specs: Vec<ToolSpec>) -> Vec<String> {
        specs.into_iter().map(|s| s.name).collect()
    }

    /// 206: a `cat:<id>` entry in a session's hide set hides every builtin of that
    /// category for the session and its subagents, one registered later too; an MCP
    /// tool whose name would fall in the category is untouched.
    #[tokio::test]
    async fn builtins_category_key_hides_natives_now_and_later_not_mcp() {
        let reg = registry(
            vec![
                Arc::new(Echo),
                Arc::new(fs::ReadFile),
                Arc::new(FilesMcp("search", Risk::ReadOnly)),
            ],
            None,
        );
        let (parent, child, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        let before = reg.ask(HiddenEpoch).await.unwrap();
        reg.ask(SetHidden {
            session: parent,
            tools: vec![
                builtins::category_key("files"),
                builtins::category_key("knowledge"),
            ],
            hidden: true,
        })
        .await
        .unwrap();
        assert_ne!(reg.ask(HiddenEpoch).await.unwrap(), before);
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo", "search"]
        );
        reg.ask(RegisterTool(Arc::new(fs::ListDir))).await.unwrap();
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo", "search"]
        );
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(other))).await.unwrap()),
            ["echo", "list_dir", "read_file", "search"]
        );
        assert_eq!(
            tool_names(reg.ask(ListTools(None)).await.unwrap()),
            ["echo", "list_dir", "read_file", "search"]
        );
        let out = reg.ask(Invoke(call("list_dir", child))).await.unwrap();
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "list_dir is turned off (Settings > Tools > Builtins)"
        );

        reg.ask(SetHidden {
            session: parent,
            tools: vec![builtins::category_key("files")],
            hidden: false,
        })
        .await
        .unwrap();
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["echo", "list_dir", "read_file", "search"]
        );
    }

    /// A registry whose `config.yaml` lives in a fresh scratch dir (returned).
    fn switchable(tools: Vec<DynTool>) -> (ActorRef<ToolRegistry>, PathBuf) {
        let dir = scratch_dir();
        let reg = ToolRegistry::spawn(ToolRegistry {
            config_path: dir.join("config.yaml"),
            ..build(tools, None)
        });
        (reg, dir)
    }

    /// [`SetBuiltinsDisabled`] with the handler's error back as a plain result.
    async fn switch(
        reg: &ActorRef<ToolRegistry>,
        names: &[&str],
        disabled: bool,
    ) -> Result<(), BuiltinsSwitchError> {
        let names = names.iter().map(|n| (*n).to_owned()).collect();
        match reg.ask(SetBuiltinsDisabled { names, disabled }).await {
            Ok(()) => Ok(()),
            Err(kameo::error::SendError::HandlerError(e)) => Err(e),
            Err(e) => panic!("registry: {e}"),
        }
    }

    /// 206: the harness-wide set hides a builtin (by name or category key) for every
    /// session, moves [`HiddenEpoch`], never touches MCP tools or `ListTools(None)`,
    /// and a call says the tool is turned off.
    #[tokio::test]
    async fn builtins_harness_wide_set_hides_for_every_session_and_bumps_epoch() {
        let (reg, dir) = switchable(vec![
            Arc::new(Echo),
            Arc::new(fs::ReadFile),
            Arc::new(FilesMcp("search", Risk::ReadOnly)),
        ]);
        let (a, b) = (SessionId::new(), SessionId::new());
        let before = reg.ask(HiddenEpoch).await.unwrap();
        switch(&reg, &["echo", "cat:files", "read_file"], true)
            .await
            .unwrap();
        // One call is one bump however many names it carries (each bump re-lists every
        // running turn; an endless chat pays a cold prefix for it).
        assert_eq!(reg.ask(HiddenEpoch).await.unwrap(), before + 1);
        for s in [a, b] {
            assert_eq!(
                tool_names(reg.ask(ListTools(Some(s))).await.unwrap()),
                ["search"]
            );
        }
        assert_eq!(
            tool_names(reg.ask(ListTools(None)).await.unwrap()),
            ["echo", "read_file", "search"]
        );
        let out = reg.ask(Invoke(call("echo", a))).await.unwrap();
        assert_eq!(
            out.content,
            "echo is turned off (Settings > Tools > Builtins)"
        );
        assert!(!reg.ask(Invoke(call("search", a))).await.unwrap().is_error);

        // Removing a registered builtin that is not in the set is fine.
        switch(&reg, &["echo", "read_file"], false).await.unwrap();
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(b))).await.unwrap()),
            ["echo", "search"]
        );
        assert!(!reg.ask(Invoke(call("echo", a))).await.unwrap().is_error);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("theuth-builtins-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 206: an unknown name, an MCP tool or an unknown category key is refused, and
    /// then nothing changes: not the set, not the file.
    #[tokio::test]
    async fn builtins_set_disabled_rejects_unknown_and_writes_nothing() {
        let (reg, dir) = switchable(vec![
            Arc::new(Echo),
            Arc::new(FilesMcp("search", Risk::ReadOnly)),
        ]);
        assert_eq!(
            switch(&reg, &["echo", "cat:nope", "search", "missing"], true).await,
            Err(BuiltinsSwitchError::Unknown(vec![
                "cat:nope".into(),
                "search".into(),
                "missing".into()
            ]))
        );
        assert!(!dir.join("config.yaml").exists());
        let cats = reg.ask(ListBuiltins(None)).await.unwrap();
        assert!(cats.iter().flat_map(|c| &c.tools).all(|t| !t.disabled));
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(SessionId::new()))).await.unwrap()),
            ["echo", "search"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 206 (B-1): a switch whose `config.yaml` cannot be written is an error naming the
    /// file, and then memory is untouched: same set, same [`HiddenEpoch`].
    #[tokio::test]
    async fn builtins_set_disabled_unwritable_config_changes_nothing() {
        let dir = scratch_dir();
        let path = dir.join("no-such-dir").join("config.yaml");
        let reg = ToolRegistry::spawn(ToolRegistry {
            config_path: path.clone(),
            ..build(vec![Arc::new(Echo)], None)
        });
        let before = reg.ask(HiddenEpoch).await.unwrap();
        let err = switch(&reg, &["echo"], true).await.unwrap_err();
        let BuiltinsSwitchError::NotWritten(why) = &err else {
            panic!("{err:?}");
        };
        assert!(why.contains(&path.display().to_string()), "{why}");
        assert!(err.to_string().contains("could not be written"), "{err}");
        assert_eq!(reg.ask(HiddenEpoch).await.unwrap(), before);
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(SessionId::new()))).await.unwrap()),
            ["echo"]
        );
        let cats = reg.ask(ListBuiltins(None)).await.unwrap();
        assert!(cats.iter().flat_map(|c| &c.tools).all(|t| !t.disabled));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 206 (B-3): a stale entry (its tool not registered now) can be removed, never added.
    #[tokio::test]
    async fn builtins_stale_entry_can_be_removed_not_added() {
        let dir = scratch_dir();
        let path = dir.join("config.yaml");
        let reg = ToolRegistry::spawn(ToolRegistry {
            config_path: path.clone(),
            builtins_disabled: ["gone_tool".to_owned()].into(),
            ..build(vec![Arc::new(Echo)], None)
        });
        assert_eq!(
            switch(&reg, &["gone_tool"], true).await,
            Err(BuiltinsSwitchError::Unknown(vec!["gone_tool".into()]))
        );
        assert_eq!(
            switch(&reg, &["never_there"], false).await,
            Err(BuiltinsSwitchError::Unknown(vec!["never_there".into()]))
        );
        switch(&reg, &["gone_tool"], false).await.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("disabled: []"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 206 (B-4): a call that changes nothing (no names, or every name already in the
    /// wanted state) neither moves [`HiddenEpoch`] nor writes the file.
    #[tokio::test]
    async fn builtins_noop_switch_keeps_epoch_and_file() {
        let (reg, dir) = switchable(vec![Arc::new(Echo)]);
        let path = dir.join("config.yaml");
        let epoch = reg.ask(HiddenEpoch).await.unwrap();
        switch(&reg, &[], true).await.unwrap();
        switch(&reg, &["echo"], false).await.unwrap();
        assert_eq!(reg.ask(HiddenEpoch).await.unwrap(), epoch);
        assert!(!path.exists());
        switch(&reg, &["echo"], true).await.unwrap();
        let epoch = reg.ask(HiddenEpoch).await.unwrap();
        std::fs::remove_file(&path).unwrap();
        switch(&reg, &["echo"], true).await.unwrap();
        switch(&reg, &[], false).await.unwrap();
        assert_eq!(reg.ask(HiddenEpoch).await.unwrap(), epoch);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 206 (B-2): the harness's own call ([`HarnessInvoke`]) runs a builtin the switch
    /// turned off, while the model's ([`Invoke`]) is refused; a hide by name still holds
    /// for both.
    #[tokio::test]
    async fn builtins_harness_invoke_skips_only_the_switch() {
        let (reg, dir) = switchable(vec![Arc::new(DocRead)]);
        let s = SessionId::new();
        switch(&reg, &["cat:documents"], true).await.unwrap();
        let out = reg.ask(Invoke(call("doc_read", s))).await.unwrap();
        assert_eq!(
            out.content,
            "doc_read is turned off (Settings > Tools > Builtins)"
        );
        let out = reg.ask(HarnessInvoke(call("doc_read", s))).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "doc_read");
        reg.ask(SetHidden {
            session: s,
            tools: vec!["doc_read".into()],
            hidden: true,
        })
        .await
        .unwrap();
        let out = reg.ask(HarnessInvoke(call("doc_read", s))).await.unwrap();
        assert_eq!(out.content, "unknown tool: doc_read");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 206: `ListBuiltins` lists natives only, by category in table order, with
    /// `disabled` from the harness-wide set and `hidden` from the session's and its
    /// ancestors' hide sets (name or category key); no session: never hidden.
    #[tokio::test]
    async fn builtins_list_flags_and_inheritance() {
        let (reg, dir) = switchable(vec![
            Arc::new(Echo),
            Arc::new(fs::ReadFile),
            Arc::new(fs::ListDir),
            Arc::new(FilesMcp("search", Risk::ReadOnly)),
        ]);
        let (parent, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        switch(&reg, &["read_file"], true).await.unwrap();
        reg.ask(SetHidden {
            session: parent,
            tools: vec![builtins::category_key("other"), "list_dir".into()],
            hidden: true,
        })
        .await
        .unwrap();
        let flags = |cats: Vec<BuiltinCategory>| {
            cats.into_iter()
                .map(|c| {
                    let tools = c
                        .tools
                        .into_iter()
                        .map(|t| (t.name, t.disabled, t.hidden))
                        .collect::<Vec<_>>();
                    (c.id, tools)
                })
                .collect::<Vec<_>>()
        };
        let s = |x: &str| x.to_owned();
        assert_eq!(
            flags(reg.ask(ListBuiltins(Some(child))).await.unwrap()),
            [
                (
                    s("files"),
                    vec![(s("list_dir"), false, true), (s("read_file"), true, false)]
                ),
                (s("other"), vec![(s("echo"), false, true)]),
            ]
        );
        assert_eq!(
            flags(reg.ask(ListBuiltins(None)).await.unwrap()),
            [
                (
                    s("files"),
                    vec![(s("list_dir"), false, false), (s("read_file"), true, false)]
                ),
                (s("other"), vec![(s("echo"), false, false)]),
            ]
        );
        switch(&reg, &["cat:files"], true).await.unwrap();
        let cats = reg.ask(ListBuiltins(None)).await.unwrap();
        assert!(cats[0].tools.iter().all(|t| t.disabled));
        assert_eq!(cats[0].label, "Files");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 206: the switch persists to `tools.builtins.disabled` (sorted, comments kept) and
    /// a registry started on that file holds the same set.
    #[tokio::test]
    async fn builtins_disabled_round_trips_through_config_yaml() {
        let dir = scratch_dir();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "# keep me\ntools:\n  approvals: true\n").unwrap();
        let reg = ToolRegistry::spawn(ToolRegistry {
            config_path: path.clone(),
            ..build(vec![Arc::new(Echo), Arc::new(fs::ReadFile)], None)
        });
        switch(&reg, &["read_file", "cat:web"], true).await.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"), "{text}");
        assert!(text.contains("approvals: true"), "{text}");

        let rt = Runtime::new(
            figment::Figment::new()
                .merge(<figment::providers::Yaml as figment::providers::Format>::file(&path)),
            dir.clone(),
        );
        let cfg = config(&rt).unwrap();
        assert!(cfg.approvals);
        assert_eq!(cfg.builtins.disabled, ["cat:web", "read_file"]);
        let restarted = ToolRegistry::spawn(ToolRegistry {
            builtins_disabled: cfg.builtins.disabled.into_iter().collect(),
            ..build(vec![Arc::new(Echo), Arc::new(fs::ReadFile)], None)
        });
        assert_eq!(
            tool_names(
                restarted
                    .ask(ListTools(Some(SessionId::new())))
                    .await
                    .unwrap()
            ),
            ["echo"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Unwrap a category command's handler `Result` instead of kameo's lifted error.
    macro_rules! category_command {
        ($reg:expr, $message:expr) => {
            match $reg.ask($message).await {
                Ok(value) => Ok(value),
                Err(kameo::error::SendError::HandlerError(e)) => Err(e),
                Err(e) => panic!("registry: {e}"),
            }
        };
    }

    fn tool_category<'a>(infos: &'a [ToolCategoryInfo], id: &str) -> &'a ToolCategoryInfo {
        infos.iter().find(|c| c.id == id).unwrap()
    }

    fn saved_categories(dir: &std::path::Path) -> ToolCategoriesConfig {
        let path = dir.join("config.yaml");
        let rt = Runtime::new(
            figment::Figment::new()
                .merge(<figment::providers::Yaml as figment::providers::Format>::file(&path)),
            dir.to_path_buf(),
        );
        config(&rt).unwrap().categories
    }

    #[tokio::test]
    async fn category_hide_covers_native_and_mcp_but_not_harness_invoke() {
        let (reg, dir) = switchable(vec![
            Arc::new(DocRead),
            Arc::new(FilesMcp("search", Risk::ReadOnly)),
        ]);
        category_command!(
            &reg,
            SetMcpCategory {
                server: "files".into(),
                category: Some("documentation".into()),
            }
        )
        .unwrap();
        let (parent, child, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        reg.ask(SetHidden {
            session: parent,
            tools: vec![categories::tool_category_key("documentation")],
            hidden: true,
        })
        .await
        .unwrap();

        assert!(tool_names(reg.ask(ListTools(Some(child))).await.unwrap()).is_empty());
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(other))).await.unwrap()),
            ["doc_read", "search"]
        );
        let out = reg.ask(Invoke(call("doc_read", child))).await.unwrap();
        assert!(out.is_error);
        let out = reg
            .ask(HarnessInvoke(call("doc_read", child)))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "doc_read");

        let infos = reg.ask(ListToolCategories(Some(child))).await.unwrap();
        let documentation = tool_category(&infos, "documentation");
        assert!(documentation.hidden);
        assert_eq!(documentation.builtin_tools, ["doc_read"]);
        assert_eq!(documentation.builtin_count, 1);
        assert_eq!(documentation.mcp_servers, ["files"]);
        let builtins = reg.ask(ListBuiltins(Some(child))).await.unwrap();
        assert!(
            builtins
                .iter()
                .flat_map(|c| &c.tools)
                .find(|t| t.name == "doc_read")
                .unwrap()
                .hidden
        );

        reg.ask(SetHidden {
            session: parent,
            tools: vec![categories::tool_category_key("documentation")],
            hidden: false,
        })
        .await
        .unwrap();
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(child))).await.unwrap()),
            ["doc_read", "search"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn custom_categories_create_update_delete_and_clear_references() {
        let (reg, dir) = switchable(vec![
            Arc::new(DocRead),
            Arc::new(FilesMcp("search", Risk::ReadOnly)),
        ]);
        let created = category_command!(
            &reg,
            UpsertToolCategory {
                id: None,
                label: "  Research Notes  ".into(),
                description: Some("Papers and notes".into()),
            }
        )
        .unwrap();
        assert_eq!(created.id, "research-notes");
        assert_eq!(created.label, "Research Notes");
        assert_eq!(created.description, "Papers and notes");
        assert!(created.custom);

        let duplicate = category_command!(
            &reg,
            UpsertToolCategory {
                id: None,
                label: "Research Notes".into(),
                description: None,
            }
        )
        .unwrap();
        assert_eq!(duplicate.id, "research-notes-2");
        let updated = category_command!(
            &reg,
            UpsertToolCategory {
                id: Some("research-notes".into()),
                label: " Knowledge Base ".into(),
                description: None,
            }
        )
        .unwrap();
        assert_eq!(updated.label, "Knowledge Base");
        assert_eq!(updated.description, "");
        assert!(
            category_command!(
                &reg,
                UpsertToolCategory {
                    id: Some("web".into()),
                    label: "Not Web".into(),
                    description: None,
                }
            )
            .unwrap_err()
            .contains("default category")
        );
        assert!(
            category_command!(
                &reg,
                UpsertToolCategory {
                    id: Some("Bad Id".into()),
                    label: "Bad".into(),
                    description: None,
                }
            )
            .is_err()
        );

        category_command!(
            &reg,
            SetMcpCategory {
                server: "files".into(),
                category: Some("research-notes".into()),
            }
        )
        .unwrap();
        category_command!(
            &reg,
            SetToolCategory {
                tool: "doc_read".into(),
                category: Some("research-notes".into()),
            }
        )
        .unwrap();
        category_command!(
            &reg,
            SetDefaultToolCategories {
                disabled: vec!["research-notes".into()],
            }
        )
        .unwrap();
        assert_eq!(
            reg.ask(DefaultHiddenCategories).await.unwrap(),
            ["tcat:research-notes"]
        );

        let before = reg.ask(HiddenEpoch).await.unwrap();
        category_command!(
            &reg,
            DeleteToolCategory {
                id: "research-notes".into(),
            }
        )
        .unwrap();
        assert_eq!(reg.ask(HiddenEpoch).await.unwrap(), before + 1);
        let infos = reg.ask(ListToolCategories(None)).await.unwrap();
        assert!(!infos.iter().any(|c| c.id == "research-notes"));
        assert_eq!(
            tool_category(&infos, "documentation").builtin_tools,
            ["doc_read"]
        );
        assert!(infos.iter().all(|c| c.mcp_servers.is_empty()));
        assert!(reg.ask(DefaultHiddenCategories).await.unwrap().is_empty());
        assert!(
            category_command!(
                &reg,
                DeleteToolCategory {
                    id: "engineering".into(),
                }
            )
            .unwrap_err()
            .contains("default category")
        );

        let cfg = saved_categories(&dir);
        assert_eq!(cfg.custom.len(), 1);
        assert_eq!(cfg.custom[0].id, "research-notes-2");
        assert!(cfg.mcp_assignments.is_empty());
        assert!(cfg.tool_assignments.is_empty());
        assert!(cfg.default_disabled.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn generated_category_ids_keep_suffixes_within_the_length_limit() {
        let (reg, dir) = switchable(Vec::new());
        let label = "x".repeat(48);
        let first = category_command!(
            &reg,
            UpsertToolCategory {
                id: None,
                label: label.clone(),
                description: None,
            }
        )
        .unwrap();
        assert_eq!(first.id, label);
        let second = category_command!(
            &reg,
            UpsertToolCategory {
                id: None,
                label,
                description: None,
            }
        )
        .unwrap();
        assert_eq!(second.id, format!("{}-2", "x".repeat(46)));
        assert_eq!(second.id.len(), 48);
        categories::validate_category_id(&second.id).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn mcp_category_assignment_changes_visibility_and_persists() {
        let (reg, dir) = switchable(vec![Arc::new(FilesMcp("search", Risk::ReadOnly))]);
        let session = SessionId::new();
        reg.ask(SetHidden {
            session,
            tools: vec![categories::tool_category_key("design")],
            hidden: true,
        })
        .await
        .unwrap();
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(session))).await.unwrap()),
            ["search"]
        );
        let before = reg.ask(HiddenEpoch).await.unwrap();
        category_command!(
            &reg,
            SetMcpCategory {
                server: "files".into(),
                category: Some("design".into()),
            }
        )
        .unwrap();
        assert_eq!(reg.ask(HiddenEpoch).await.unwrap(), before + 1);
        assert!(tool_names(reg.ask(ListTools(Some(session))).await.unwrap()).is_empty());
        assert_eq!(
            tool_category(&reg.ask(ListToolCategories(None)).await.unwrap(), "design").mcp_servers,
            ["files"]
        );

        category_command!(
            &reg,
            SetMcpCategory {
                server: "files".into(),
                category: None,
            }
        )
        .unwrap();
        assert_eq!(
            tool_names(reg.ask(ListTools(Some(session))).await.unwrap()),
            ["search"]
        );
        assert!(
            category_command!(
                &reg,
                SetMcpCategory {
                    server: "files".into(),
                    category: Some("missing".into()),
                }
            )
            .is_err()
        );
        let cfg = saved_categories(&dir);
        assert!(cfg.mcp_assignments.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn native_tool_category_override_moves_tool_and_can_return_to_default() {
        let (reg, dir) = switchable(vec![Arc::new(DocRead)]);
        category_command!(
            &reg,
            SetToolCategory {
                tool: "doc_read".into(),
                category: Some("design".into()),
            }
        )
        .unwrap();
        let infos = reg.ask(ListToolCategories(None)).await.unwrap();
        assert_eq!(tool_category(&infos, "design").builtin_tools, ["doc_read"]);
        assert!(
            tool_category(&infos, "documentation")
                .builtin_tools
                .is_empty()
        );

        let session = SessionId::new();
        reg.ask(SetHidden {
            session,
            tools: vec![categories::tool_category_key("design")],
            hidden: true,
        })
        .await
        .unwrap();
        assert!(
            reg.ask(Invoke(call("doc_read", session)))
                .await
                .unwrap()
                .is_error
        );
        category_command!(
            &reg,
            SetToolCategory {
                tool: "doc_read".into(),
                category: Some("documentation".into()),
            }
        )
        .unwrap();
        assert!(
            !reg.ask(Invoke(call("doc_read", session)))
                .await
                .unwrap()
                .is_error
        );
        let infos = reg.ask(ListToolCategories(None)).await.unwrap();
        assert_eq!(
            tool_category(&infos, "documentation").builtin_tools,
            ["doc_read"]
        );
        assert!(
            category_command!(
                &reg,
                SetToolCategory {
                    tool: "missing".into(),
                    category: Some("web".into()),
                }
            )
            .is_err()
        );
        // Clearing is allowed for a stale tool name.
        category_command!(
            &reg,
            SetToolCategory {
                tool: "missing".into(),
                category: None,
            }
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn default_disabled_categories_are_validated_sorted_and_persisted() {
        let (reg, dir) = switchable(Vec::new());
        category_command!(
            &reg,
            SetDefaultToolCategories {
                disabled: vec!["web".into(), "design".into(), "web".into()],
            }
        )
        .unwrap();
        assert_eq!(
            reg.ask(DefaultHiddenCategories).await.unwrap(),
            ["tcat:design", "tcat:web"]
        );
        let infos = reg.ask(ListToolCategories(None)).await.unwrap();
        assert!(!tool_category(&infos, "design").default_enabled);
        assert!(!tool_category(&infos, "web").default_enabled);
        assert!(tool_category(&infos, "engineering").default_enabled);
        assert!(infos.iter().all(|c| !c.hidden));
        let cfg = saved_categories(&dir);
        assert_eq!(cfg.default_disabled, ["design", "web"]);

        assert!(
            category_command!(
                &reg,
                SetDefaultToolCategories {
                    disabled: vec!["missing".into()],
                }
            )
            .is_err()
        );
        assert_eq!(
            reg.ask(DefaultHiddenCategories).await.unwrap(),
            ["tcat:design", "tcat:web"]
        );
        category_command!(
            &reg,
            SetDefaultToolCategories {
                disabled: Vec::new()
            }
        )
        .unwrap();
        assert!(reg.ask(DefaultHiddenCategories).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
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
            vec![
                Arc::new(Echo),
                Arc::new(Touch),
                Arc::new(Peer(Arc::default())),
            ],
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
        assert!(
            !reg.ask(Invoke(call("touch", child)))
                .await
                .unwrap()
                .is_error
        );

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
        assert_eq!(
            out.content,
            "touch: denied: subagent is capped at read_only"
        );
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
        assert_eq!(
            out.content,
            "touch: denied: subagent is capped at read_only"
        );

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
        assert!(
            !reg.ask(Invoke(call("touch", grandchild)))
                .await
                .unwrap()
                .is_error
        );
    }

    /// A `ReadOnly` tool whose call is credentialed when its args say
    /// `"signed": true`, like a browser click on a profile site (209 D14).
    struct SignedIn;

    #[async_trait::async_trait]
    impl Tool for SignedIn {
        const NAME: &'static str = "signed_in";
        const RISK: Risk = Risk::ReadOnly;
        const CREDENTIALED: bool = true;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        fn credentialed(&self, _: SessionId, args: &Self::Args) -> bool {
            args["signed"] == true
        }
        async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok("acted"))
        }
    }

    /// 209 D14: a credentialed call is gated as `Mutating` (inspection and
    /// policy see it), a session capped at `read_only` is refused it with
    /// the cap's text, and the same tool's uncredentialed calls stay
    /// `ReadOnly` and run.
    #[tokio::test]
    async fn credentialed_call_is_raised_to_mutating_and_a_read_only_cap_denies_it() {
        let policy = Policy {
            read_only: PolicyAction::Allow,
            mutating: PolicyAction::Ask,
            destructive: PolicyAction::Deny,
        };
        let reg = registry_with(vec![Arc::new(SignedIn)], None, policy, None);
        let (parent, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        let with = |session, signed: bool| ToolCall {
            args: serde_json::json!({ "signed": signed }),
            ..call("signed_in", session)
        };
        let inspect = |signed: bool| InspectTool {
            session: parent,
            name: "signed_in".into(),
            args: Some(serde_json::json!({ "signed": signed })),
        };

        let raised = reg.ask(inspect(true)).await.unwrap();
        assert_eq!(raised.spec.risk, Risk::Mutating);
        assert_eq!(raised.action, PolicyAction::Ask);
        let plain = reg.ask(inspect(false)).await.unwrap();
        assert_eq!(plain.spec.risk, Risk::ReadOnly);
        assert_eq!(plain.action, PolicyAction::Allow);

        // Uncapped: the credentialed call asks (no approval actor here, so
        // it fails asking); the plain one runs.
        let out = reg.ask(Invoke(with(parent, true))).await.unwrap();
        assert!(out.is_error && out.content.contains("approval"), "{out:?}");
        let out = reg.ask(Invoke(with(parent, false))).await.unwrap();
        assert_eq!(out.content, "acted");

        reg.tell(SetCap {
            session: child,
            cap: Some(Risk::ReadOnly),
        })
        .await
        .unwrap();
        let out = reg.ask(Invoke(with(child, true))).await.unwrap();
        assert_eq!(
            out.content,
            "signed_in: denied: subagent is capped at read_only"
        );
        let out = reg.ask(Invoke(with(child, false))).await.unwrap();
        assert_eq!(out.content, "acted");
    }

    /// A `Mutating` tool whose card is the approval, like `browser_show_element` (209 §9).
    struct ByCard;

    #[async_trait::async_trait]
    impl Tool for ByCard {
        const NAME: &'static str = "by_card";
        const RISK: Risk = Risk::Mutating;
        const APPROVED_BY_CARD: bool = true;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok("carded"))
        }
    }

    /// 209 §9: with mutating calls asking and prompts on, a tool approved by its card runs
    /// without a prompt (a plain mutating tool still asks), while a session capped at
    /// `read_only` neither sees it nor may call it.
    #[tokio::test]
    async fn approved_by_card_skips_the_prompt_but_not_the_cap() {
        let policy = Policy {
            read_only: PolicyAction::Allow,
            mutating: PolicyAction::Ask,
            destructive: PolicyAction::Deny,
        };
        let reg = registry_with(
            vec![Arc::new(ByCard), Arc::new(Touch), Arc::new(Echo)],
            None,
            policy,
            None,
        );
        let (parent, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        let out = reg.ask(Invoke(call("by_card", parent))).await.unwrap();
        assert_eq!(out.content, "carded");
        let out = reg.ask(Invoke(call("touch", parent))).await.unwrap();
        assert!(out.is_error && out.content.contains("approval"), "{out:?}");
        let described = reg.ask(DescribeTools(Some(parent))).await.unwrap();
        let card = described
            .iter()
            .find(|i| i.spec.name == "by_card")
            .expect("listed uncapped");
        assert_eq!(
            (card.spec.risk, card.action),
            (Risk::Mutating, PolicyAction::Allow)
        );

        reg.tell(SetCap {
            session: child,
            cap: Some(Risk::ReadOnly),
        })
        .await
        .unwrap();
        let names: Vec<String> = reg
            .ask(ListTools(Some(child)))
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["echo"]);
        let out = reg.ask(Invoke(call("by_card", child))).await.unwrap();
        assert_eq!(
            out.content,
            "by_card: denied: subagent is capped at read_only"
        );
    }

    /// Journal that records every `approvals` write.
    struct PinLog(Mutex<Vec<(SessionId, Option<bool>)>>);

    impl Journal for PinLog {
        fn call_started(&self, _: SessionId, _: &str) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn call_finished(
            &self,
            _: SessionId,
            _: &str,
            _: &ToolOutput,
        ) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn question(&self, _: QuestionRow) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn wait(
            &self,
            _: Option<crate::WaitRow>,
            _: SessionId,
            _: &str,
        ) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn job(&self, _: crate::JobRow) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn pty_screen(&self, _: crate::PtyScreenRow) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn approvals(&self, s: SessionId, pin: Option<bool>) -> futures::future::BoxFuture<'_, ()> {
            // Recorded when awaited: a write left to a spawned task lands late.
            Box::pin(async move { lock(&self.0).push((s, pin)) })
        }
    }

    /// What `wipe` (destructive: `ask` by policy) resolves to for `session`.
    async fn wipe_action(reg: &ActorRef<ToolRegistry>, session: SessionId) -> PolicyAction {
        reg.ask(InspectTool {
            session,
            name: "wipe".into(),
            args: Some(serde_json::json!({"target": "db"})),
        })
        .await
        .unwrap()
        .action
    }

    async fn pin_session(reg: &ActorRef<ToolRegistry>, session: SessionId, on: bool, clear: bool) {
        reg.ask(SetApprovals {
            session: Some(session),
            enabled: on,
            clear,
        })
        .await
        .unwrap();
    }

    /// 183 §4.5 (W-4): a session pin beats `tools.approvals` both ways, a
    /// child inherits the nearest ancestor's pin, `clear` goes back to the
    /// global switch, and `ApprovalsState.pin` is the session's own pin.
    #[tokio::test]
    async fn session_pin_beats_the_global_switch_and_children_inherit_it() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (parent, child, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        for global in [true, false] {
            let reg = ToolRegistry::spawn(ToolRegistry {
                approvals: global,
                ..build(vec![Arc::new(Wipe(runs.clone()))], None)
            });
            reg.tell(SetParent {
                session: child,
                parent,
            })
            .await
            .unwrap();
            let inherit = if global {
                PolicyAction::Ask
            } else {
                PolicyAction::Allow
            };
            assert_eq!(wipe_action(&reg, parent).await, inherit);

            pin_session(&reg, parent, true, false).await;
            assert_eq!(wipe_action(&reg, parent).await, PolicyAction::Ask);
            assert_eq!(
                wipe_action(&reg, child).await,
                PolicyAction::Ask,
                "inherited"
            );
            assert_eq!(wipe_action(&reg, other).await, inherit, "unrelated session");
            let st = reg
                .ask(GetApprovals {
                    session: Some(child),
                })
                .await
                .unwrap();
            assert_eq!((st.global, st.session, st.pin), (global, true, None));

            // The nearest pin wins: the child's own `off` beats the parent's `on`.
            pin_session(&reg, child, false, false).await;
            assert_eq!(wipe_action(&reg, child).await, PolicyAction::Allow);
            assert_eq!(wipe_action(&reg, parent).await, PolicyAction::Ask);
            let st = reg
                .ask(GetApprovals {
                    session: Some(child),
                })
                .await
                .unwrap();
            assert_eq!((st.session, st.pin), (false, Some(false)));

            pin_session(&reg, child, true, true).await;
            pin_session(&reg, parent, true, true).await;
            assert_eq!(
                wipe_action(&reg, child).await,
                inherit,
                "cleared: back to global"
            );
            let st = reg
                .ask(GetApprovals {
                    session: Some(parent),
                })
                .await
                .unwrap();
            assert_eq!((st.session, st.pin), (global, None));

            // `off` with approvals on: asks become allow.
            pin_session(&reg, parent, false, false).await;
            assert_eq!(wipe_action(&reg, parent).await, PolicyAction::Allow);
        }
    }

    /// 183 §4.5: a cap still denies a session pinned `on`; `Deny` is never rewritten.
    #[tokio::test]
    async fn pin_never_rewrites_a_cap_denial() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reg = registry(vec![Arc::new(Wipe(runs))], None);
        let session = SessionId::new();
        reg.tell(SetCap {
            session,
            cap: Some(Risk::Mutating),
        })
        .await
        .unwrap();
        pin_session(&reg, session, true, false).await;
        assert_eq!(wipe_action(&reg, session).await, PolicyAction::Deny);
    }

    /// 183 §4.5 (D4): `SetApprovals` with a session writes the pin through the
    /// installed journal (`clear` → `None`) before it replies, so a reader that
    /// re-applies the row sees the new pin; `RestoreApprovals` applies a loaded
    /// pin without writing it back; the global write touches no row.
    #[tokio::test]
    async fn session_pin_is_journaled_and_restore_is_not() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let log = Arc::new(PinLog(Mutex::default()));
        let reg = ToolRegistry::spawn(ToolRegistry {
            journal: log.clone(),
            ..build(vec![Arc::new(Wipe(runs))], None)
        });
        let session = SessionId::new();
        pin_session(&reg, session, false, false).await;
        assert_eq!(
            *lock(&log.0),
            [(session, Some(false))],
            "written before the reply"
        );
        pin_session(&reg, session, true, true).await;
        reg.ask(SetApprovals {
            session: None,
            enabled: false,
            clear: false,
        })
        .await
        .unwrap();
        reg.ask(RestoreApprovals {
            session,
            pin: Some(true),
        })
        .await
        .unwrap();
        assert_eq!(
            wipe_action(&reg, session).await,
            PolicyAction::Ask,
            "restored"
        );
        assert_eq!(*lock(&log.0), [(session, Some(false)), (session, None)]);
    }

    /// 136 R-4: `approvals: false` turns every `ask` into `allow`, so a
    /// `SetCap{Mutating}` is what keeps destructive tools out of an
    /// unattended run: the capped-and-bypassed session runs a mutating tool
    /// without a prompt and is refused a destructive one.
    #[tokio::test]
    async fn cap_holds_for_a_session_with_approvals_off() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reg = registry(
            vec![
                Arc::new(Wipe(runs.clone())),
                Arc::new(Touch),
                Arc::new(Echo),
            ],
            None,
        );
        let session = SessionId::new();
        reg.ask(SetApprovals {
            session: Some(session),
            enabled: false,
            clear: false,
        })
        .await
        .unwrap();
        reg.tell(SetCap {
            session,
            cap: Some(Risk::Mutating),
        })
        .await
        .unwrap();
        let out = reg.ask(Invoke(call("touch", session))).await.unwrap();
        assert!(!out.is_error, "mutating runs without a prompt: {out:?}");
        let out = reg
            .ask(Invoke(app_call(
                "wipe",
                session,
                serde_json::json!({"target": "db"}),
            )))
            .await
            .unwrap();
        assert_eq!(out.content, "wipe: denied: subagent is capped at mutating");
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
        let names: Vec<String> = reg
            .ask(ListTools(Some(session)))
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(
            names,
            ["echo", "touch"],
            "the destructive tool is not offered"
        );
    }

    /// 183 W-39: a contact's `read_only` turn cap refuses a mutating tool with approvals off,
    /// the stricter of turn cap and session cap wins, and clearing the turn cap leaves the
    /// session's own `SetCap` as it was. Z3-R3: the turn cap is the turn's own session's — a
    /// child the owner started keeps its tools (one the turn spawns gets its own `SetCap`).
    #[tokio::test]
    async fn turn_cap_refuses_above_it_and_never_replaces_the_session_cap() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reg = registry(
            vec![
                Arc::new(Wipe(runs.clone())),
                Arc::new(Touch),
                Arc::new(Echo),
            ],
            None,
        );
        let (chat, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent: chat,
        })
        .await
        .unwrap();
        reg.ask(SetApprovals {
            session: Some(chat),
            enabled: false,
            clear: false,
        })
        .await
        .unwrap();
        reg.tell(SetCap {
            session: chat,
            cap: Some(Risk::Mutating),
        })
        .await
        .unwrap();
        reg.tell(SetTurnCap {
            session: chat,
            cap: Some(Risk::ReadOnly),
        })
        .await
        .unwrap();
        let out = reg.ask(Invoke(call("touch", chat))).await.unwrap();
        assert_eq!(
            out.content,
            "touch: denied: subagent is capped at read_only"
        );
        let out = reg.ask(Invoke(call("echo", chat))).await.unwrap();
        assert!(!out.is_error, "read-only runs: {out:?}");
        let out = reg.ask(Invoke(call("touch", child))).await.unwrap();
        assert!(!out.is_error, "the owner's child keeps its tools: {out:?}");
        // A looser turn cap than the session's does not loosen it.
        reg.tell(SetTurnCap {
            session: chat,
            cap: Some(Risk::Destructive),
        })
        .await
        .unwrap();
        let out = reg
            .ask(Invoke(app_call(
                "wipe",
                chat,
                serde_json::json!({"target": "db"}),
            )))
            .await
            .unwrap();
        assert_eq!(out.content, "wipe: denied: subagent is capped at mutating");
        // The turn ends: the session's own cap is what remains.
        reg.tell(SetTurnCap {
            session: chat,
            cap: None,
        })
        .await
        .unwrap();
        let out = reg.ask(Invoke(call("touch", chat))).await.unwrap();
        assert!(!out.is_error, "mutating runs again: {out:?}");
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
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

    /// Read-only stand-ins under a grantable and an ungrantable name.
    macro_rules! read_only_tool {
        ($ty:ident, $name:literal) => {
            struct $ty;

            #[async_trait::async_trait]
            impl Tool for $ty {
                const NAME: &'static str = $name;
                const RISK: Risk = Risk::ReadOnly;
                type Args = serde_json::Value;
                fn description(&self) -> String {
                    String::new()
                }
                async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
                    Ok(ToolOutput::ok($name))
                }
            }
        };
    }
    read_only_tool!(ReadUrl, "read_url");
    read_only_tool!(BrowserOpen, "browser_open");
    read_only_tool!(DocRead, "doc_read");

    /// An MCP tool of server `files` at a given risk.
    struct FilesMcp(&'static str, Risk);

    impl crate::ErasedTool for FilesMcp {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.0.into(),
                description: String::new(),
                schema: serde_json::json!({}),
                risk: self.1,
                direct: false,
                returns: String::new(),
            }
        }
        fn source(&self) -> ToolSource {
            ToolSource::Mcp {
                server: "files".into(),
                entry: crate::McpEntry::Http {
                    url: "http://127.0.0.1/".into(),
                },
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

    /// 183 Z3-R2: a contact session is default-deny. Its allowlist is the empty list (191
    /// D5): no tool exists for the session and its subagents, the session's own or the turn's.
    #[tokio::test]
    async fn contact_sessions_see_only_their_allowlist() {
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let allow = Policy {
            mutating: PolicyAction::Allow,
            ..Policy::default()
        };
        let reg = registry_with(
            vec![
                Arc::new(Peer(ran.clone())),
                Arc::new(Echo),
                Arc::new(Touch),
                Arc::new(ReadUrl),
            ],
            None,
            allow,
            None,
        );
        reg.ask(InstallCommunicationAccess(Switch::new(true)))
            .await
            .unwrap();
        let names = |specs: Vec<ToolSpec>| specs.into_iter().map(|s| s.name).collect::<Vec<_>>();
        let everything = ["echo", "read_url", "send_message", "touch"];
        let (inbox, child, chat) = (SessionId::new(), SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent: inbox,
        })
        .await
        .unwrap();

        // The inbox: nothing exists, for it and its subagent.
        reg.tell(SetAllowOnly {
            session: inbox,
            tools: Some(Vec::new()),
            turn: false,
        })
        .await
        .unwrap();
        for s in [inbox, child] {
            assert!(
                names(reg.ask(ListTools(Some(s))).await.unwrap()).is_empty(),
                "an empty allowlist hides every tool"
            );
            for hidden in ["echo", "touch", "send_message", "read_url"] {
                let out = reg.ask(Invoke(call(hidden, s))).await.unwrap();
                assert!(
                    out.is_error && out.content.contains("unknown tool"),
                    "{out:?}"
                );
            }
        }
        // The loop's own tools (`python`, `todo`) obey the allowlist too.
        let locals = || {
            vec![
                "python".to_owned(),
                "todo".to_owned(),
                "read_url".to_owned(),
            ]
        };
        assert!(
            reg.ask(Allowlisted(child, locals()))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            reg.ask(Allowlisted(chat, locals())).await.unwrap(),
            locals()
        );

        // An exported chat's contact turn: nothing exists in it, `read_url` included.
        reg.tell(SetTurnCap {
            session: chat,
            cap: Some(Risk::Mutating),
        })
        .await
        .unwrap();
        reg.tell(SetAllowOnly {
            session: chat,
            tools: Some(Vec::new()),
            turn: true,
        })
        .await
        .unwrap();
        assert!(
            names(reg.ask(ListTools(Some(chat))).await.unwrap()).is_empty(),
            "an empty allowlist hides every tool"
        );
        for hidden in ["read_url", "send_message", "touch"] {
            let out = reg.ask(Invoke(call(hidden, chat))).await.unwrap();
            assert!(out.content.contains("unknown tool"), "{out:?}");
        }
        // Z3-R3: the turn's allowlist is the turn's own session's; a subagent the owner
        // started before keeps its tools.
        let bg = SessionId::new();
        reg.tell(SetParent {
            session: bg,
            parent: chat,
        })
        .await
        .unwrap();
        let loop_tools = || vec!["python".to_owned(), "todo".to_owned()];
        assert!(
            reg.ask(Allowlisted(chat, loop_tools()))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            reg.ask(Allowlisted(bg, loop_tools())).await.unwrap(),
            loop_tools()
        );
        assert!(reg.ask(ContactScoped(chat)).await.unwrap());
        assert!(!reg.ask(ContactScoped(bg)).await.unwrap());
        // Nor does the turn's cap reach it.
        assert_eq!(reg.ask(EffectiveCap(bg)).await.unwrap(), None);
        assert_eq!(
            names(reg.ask(ListTools(Some(bg))).await.unwrap()),
            everything
        );
        // A child spawned now takes this ceiling as its own (M4).
        assert_eq!(
            reg.ask(EffectiveCap(chat)).await.unwrap(),
            Some(Risk::Mutating)
        );
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 0);

        // The turn ends: the chat is the owner's again.
        reg.tell(SetAllowOnly {
            session: chat,
            tools: None,
            turn: true,
        })
        .await
        .unwrap();
        reg.tell(SetTurnCap {
            session: chat,
            cap: None,
        })
        .await
        .unwrap();
        assert_eq!(
            names(reg.ask(ListTools(Some(chat))).await.unwrap()),
            everything
        );
        assert_eq!(
            reg.ask(Allowlisted(chat, locals())).await.unwrap(),
            locals()
        );
    }

    /// 16 §5 (2026-11-11): restricted (capped) contact states stay on the public web: a capped
    /// contact turn in a shared chat, a capped inbox and its subagent run with `egress::lan()`
    /// false; the chat is the owner's again once the turn ends; a caller outside any registry
    /// scope is treated as restricted. Full shares keep the LAN (Sky's ruling, 20:28).
    #[tokio::test]
    async fn restricted_contact_states_stay_off_the_local_network() {
        struct Probe;
        #[async_trait::async_trait]
        impl Tool for Probe {
            const NAME: &'static str = "read_url";
            const RISK: Risk = Risk::ReadOnly;
            type Args = serde_json::Value;
            fn description(&self) -> String {
                String::new()
            }
            async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
                Ok(ToolOutput::ok(crate::egress::lan().to_string()))
            }
        }
        let (owner, inbox, child, chat) = (
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
        );
        // Seeded directly: [`SetAllowOnly`] installs only the empty list (191 D5), which hides
        // the probe; the LAN rule reads the restricted state, not the list.
        let listed = || HashSet::from(["read_url".to_owned()]);
        let reg = ToolRegistry::spawn(ToolRegistry {
            allow_only: HashMap::from([(inbox, listed())]),
            turn_allow_only: HashMap::from([(chat, listed())]),
            ..build(vec![Arc::new(Probe)], None)
        });
        let lan = |s: SessionId| {
            let reg = reg.clone();
            async move { reg.ask(Invoke(call("read_url", s))).await.unwrap().content }
        };
        reg.tell(SetParent {
            session: child,
            parent: inbox,
        })
        .await
        .unwrap();
        assert_eq!(lan(owner).await, "true");
        for s in [inbox, child, chat] {
            assert_eq!(lan(s).await, "false", "{s}");
        }
        reg.tell(SetAllowOnly {
            session: chat,
            tools: None,
            turn: true,
        })
        .await
        .unwrap();
        assert_eq!(lan(chat).await, "true");
        assert!(!crate::egress::lan());

        // Full shares (190): no allowlist, no cap. Sky, 2026-11-11 20:28: a full-share contact
        // turn or full inbox runs with the agent's own tools and ceiling, so the LAN works there
        // exactly as in the owner's turns (the setting is the fetcher's half). Only a plugin's
        // marker ([`SetNoLan`]: an ingest job without the grant) takes it away.
        let (full_inbox, full_child, job) = (SessionId::new(), SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: full_child,
            parent: full_inbox,
        })
        .await
        .unwrap();
        for s in [full_inbox, full_child, chat] {
            assert_eq!(lan(s).await, "true", "{s}");
        }
        let marker = |on| SetNoLan { session: job, on };
        reg.ask(marker(true)).await.unwrap();
        assert_eq!(lan(job).await, "false");
        reg.ask(marker(false)).await.unwrap();
        assert_eq!(lan(job).await, "true");
    }

    /// 191 (D5): [`SetAllowOnly`] installs only the empty allowlist, whoever sent it: a
    /// non-empty list (a paired engine's, once) is installed as the empty list, never skipped
    /// (the session stays `contact_scoped`) and never as given (nothing is grantable, not
    /// `read_url`, not an MCP tool under its `mcp:<server>/<tool>` key).
    #[tokio::test]
    async fn a_non_empty_allowlist_is_installed_empty() {
        let reg = registry_with(
            vec![
                Arc::new(ReadUrl),
                Arc::new(BrowserOpen),
                Arc::new(Echo),
                Arc::new(FilesMcp("search", Risk::ReadOnly)),
                Arc::new(FilesMcp("delete", Risk::Mutating)),
            ],
            None,
            Policy::default(),
            None,
        );
        let names = |specs: Vec<ToolSpec>| specs.into_iter().map(|s| s.name).collect::<Vec<_>>();
        let origin = [
            "read_url",
            "browser_open",
            "echo",
            "search",
            "mcp:files/search",
            "mcp:files/delete",
        ]
        .map(String::from)
        .to_vec();
        for turn in [false, true] {
            let s = SessionId::new();
            reg.tell(SetAllowOnly {
                session: s,
                tools: Some(origin.clone()),
                turn,
            })
            .await
            .unwrap();
            assert!(
                reg.ask(ContactScoped(s)).await.unwrap(),
                "still a contact session, turn {turn}"
            );
            assert!(
                names(reg.ask(ListTools(Some(s))).await.unwrap()).is_empty(),
                "turn {turn}"
            );
            for hidden in ["read_url", "browser_open", "echo", "search", "delete"] {
                let out = reg.ask(Invoke(call(hidden, s))).await.unwrap();
                assert!(out.content.contains("unknown tool"), "{hidden}: {out:?}");
            }
        }
    }

    /// Stands in for `crawl`: answers with the arguments it was called with.
    struct Crawl;

    #[async_trait::async_trait]
    impl Tool for Crawl {
        const NAME: &'static str = "crawl";
        const RISK: Risk = Risk::ReadOnly;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, a: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok(a.to_string()))
        }
    }

    /// 183 Z4-R1: in a contact session `crawl` runs at most 20 pages and depth 2 whatever
    /// the contact asked (absent or junk too); the owner's own call is passed as sent. The
    /// allowlist is seeded directly: [`SetAllowOnly`] installs only the empty list (191 D5),
    /// which hides `crawl` altogether, so the bound is the second line behind it.
    #[tokio::test]
    async fn a_contact_crawl_is_bounded_per_call() {
        let (inbox, owner) = (SessionId::new(), SessionId::new());
        let reg = ToolRegistry::spawn(ToolRegistry {
            allow_only: HashMap::from([(inbox, HashSet::from(["crawl".to_owned()]))]),
            ..build(vec![Arc::new(Crawl)], None)
        });
        let crawl = |s, args| ToolCall {
            args,
            ..call("crawl", s)
        };
        let ran =
            |out: ToolOutput| serde_json::from_str::<serde_json::Value>(&out.content).unwrap();
        let asked = serde_json::json!({"url": "https://x.example", "limit": 5000, "max_depth": 9});
        let out = reg.ask(Invoke(crawl(inbox, asked.clone()))).await.unwrap();
        assert_eq!(
            ran(out),
            serde_json::json!({"url": "https://x.example", "limit": 20, "max_depth": 2})
        );
        let small = serde_json::json!({"url": "u", "limit": 3, "max_depth": "deep"});
        let out = reg.ask(Invoke(crawl(inbox, small))).await.unwrap();
        assert_eq!(
            ran(out),
            serde_json::json!({"url": "u", "limit": 3, "max_depth": 2})
        );
        let out = reg
            .ask(Invoke(crawl(inbox, serde_json::json!({"url": "u"}))))
            .await
            .unwrap();
        assert_eq!(
            ran(out),
            serde_json::json!({"url": "u", "limit": 20, "max_depth": 2})
        );
        let out = reg.ask(Invoke(crawl(owner, asked.clone()))).await.unwrap();
        assert_eq!(ran(out), asked);
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
            clear: false,
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

    /// Bus subscriber standing in for the human who leaves every call to the
    /// agent (207 Attention dismiss).
    #[derive(kameo::Actor)]
    struct Dismisser(ActorRef<Approval>);

    impl Message<crate::approval::ApprovalRequested> for Dismisser {
        type Reply = ();
        async fn handle(
            &mut self,
            m: crate::approval::ApprovalRequested,
            _: &mut Context<Self, ()>,
        ) {
            let _ = self.0.ask(crate::approval::Dismiss { id: m.id }).await;
        }
    }

    /// 207: a call whose approval the user left to the agent is not run and
    /// tells the model so, distinct from a denial or a timeout.
    #[tokio::test]
    async fn dismissed_approval_is_not_run() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let approval = Approval::spawn(Approval::new(rt.clone(), Duration::from_secs(5)));
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reg = registry_with(
            vec![Arc::new(Wipe(runs.clone()))],
            None,
            Policy::default(),
            Some(approval.clone()),
        );
        let dismisser = Dismisser::spawn(Dismisser(approval));
        rt.subscribe::<Dismisser, crate::approval::ApprovalRequested>(&dismisser)
            .await;
        let out = reg
            .ask(Invoke(app_call(
                "wipe",
                SessionId::new(),
                serde_json::json!({"target": "db"}),
            )))
            .await
            .unwrap();
        assert!(out.is_error, "{out:?}");
        assert_eq!(
            out.content,
            "wipe: not run — the user left this call to you without approving it. Do not retry it as is; find a way that needs no approval, or say in your reply why it is needed."
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// 207 (E1): an external harness's permission ask that the user left to
    /// the agent comes back marked `dismissed`, not as a bare deny.
    #[tokio::test]
    async fn dismissed_external_approval_is_marked_dismissed() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let approval = Approval::spawn(Approval::new(rt.clone(), Duration::from_secs(5)));
        let reg = registry_with(Vec::new(), None, Policy::default(), Some(approval.clone()));
        let dismisser = Dismisser::spawn(Dismisser(approval));
        rt.subscribe::<Dismisser, crate::approval::ApprovalRequested>(&dismisser)
            .await;
        let v = reg
            .ask(ExternalApproval {
                session: SessionId::new(),
                tool: "Delete files".into(),
                args: serde_json::json!({"kind": "delete"}),
                risk: Risk::Destructive,
            })
            .await
            .unwrap();
        assert_eq!(
            v,
            ExternalVerdict {
                decision: Decision::Deny,
                dismissed: true
            }
        );
    }

    /// A human-only tool, like `share_publish` (210 §7.2).
    struct Publish(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl Tool for Publish {
        const NAME: &'static str = "publish";
        const RISK: Risk = Risk::Destructive;
        const HUMAN_ONLY: bool = true;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolOutput::ok("published"))
        }
    }

    /// What [`Owner`] saw: per card, `(human_only, double's ResolveBy accepted)`.
    type Seen = Arc<Mutex<Vec<(bool, bool)>>>;

    /// Bus subscriber standing in for the double, then the owner: it first tries to answer
    /// every card as the double, then allows it as the human.
    #[derive(kameo::Actor)]
    struct Owner(ActorRef<Approval>, Seen);

    impl Message<crate::approval::ApprovalRequested> for Owner {
        type Reply = ();
        async fn handle(
            &mut self,
            m: crate::approval::ApprovalRequested,
            _: &mut Context<Self, ()>,
        ) {
            let double = self
                .0
                .ask(crate::approval::ResolveBy {
                    id: m.id,
                    decision: Decision::Allow,
                    by: crate::journal::Answerer::Double,
                })
                .await
                .unwrap();
            lock(&self.1).push((m.human_only, double));
            if !double {
                let _ = self
                    .0
                    .ask(crate::approval::Resolve {
                        id: m.id,
                        decision: Decision::Allow,
                    })
                    .await;
            }
        }
    }

    /// 210 §7.2: with every policy `allow`, `tools.approvals: false` and the session pinned
    /// off, a human-only call still asks; its card says `human_only`; the double's answer is
    /// refused and only the human's runs it. A plain tool under the same switches never asks.
    #[tokio::test]
    async fn human_only_asks_with_approvals_off_and_the_double_cannot_answer() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let approval = Approval::spawn(Approval::new(rt.clone(), Duration::from_secs(5)));
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let all_allow = Policy {
            read_only: PolicyAction::Allow,
            mutating: PolicyAction::Allow,
            destructive: PolicyAction::Allow,
        };
        let reg = ToolRegistry::spawn(ToolRegistry {
            policy: all_allow,
            approvals: false,
            approval: Some(approval.clone()),
            ..build(vec![Arc::new(Publish(runs.clone())), Arc::new(Echo)], None)
        });
        let seen: Seen = Arc::default();
        let owner = Owner::spawn(Owner(approval.clone(), seen.clone()));
        rt.subscribe::<Owner, crate::approval::ApprovalRequested>(&owner)
            .await;
        let session = SessionId::new();
        reg.ask(SetApprovals {
            session: Some(session),
            enabled: false,
            clear: false,
        })
        .await
        .unwrap();

        let out = reg.ask(Invoke(call("echo", session))).await.unwrap();
        assert_eq!(out.content, "hi");
        assert!(lock(&seen).is_empty(), "a plain tool asked");

        let out = reg.ask(Invoke(call("publish", session))).await.unwrap();
        assert_eq!(out.content, "published", "{out:?}");
        assert_eq!(*lock(&seen), [(true, false)]);
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        let i = reg
            .ask(InspectTool {
                session,
                name: "publish".into(),
                args: None,
            })
            .await
            .unwrap();
        assert_eq!(i.action, PolicyAction::Ask);
    }

    /// 216 §10.3: a brokered call follows the tool's own policy — `allow` runs without a card —
    /// and an `ask` raises a human-only card the double cannot answer.
    #[tokio::test]
    async fn invoke_human_only_follows_policy_and_asks_human_only() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let approval = Approval::spawn(Approval::new(rt.clone(), Duration::from_secs(5)));
        let policy = |read_only| Policy {
            read_only,
            mutating: PolicyAction::Ask,
            destructive: PolicyAction::Ask,
        };
        let seen: Seen = Arc::default();
        let owner = Owner::spawn(Owner(approval.clone(), seen.clone()));
        rt.subscribe::<Owner, crate::approval::ApprovalRequested>(&owner)
            .await;
        let brokered = |name: &str| InvokeHumanOnly {
            call: call(name, SessionId::new()),
            requester: "plugin turns".into(),
        };
        let allow = ToolRegistry::spawn(ToolRegistry {
            policy: policy(PolicyAction::Allow),
            approval: Some(approval.clone()),
            ..build(vec![Arc::new(Echo)], None)
        });
        assert_eq!(allow.ask(brokered("echo")).await.unwrap().content, "hi");
        assert!(lock(&seen).is_empty(), "allowed by policy, yet asked");

        let ask = ToolRegistry::spawn(ToolRegistry {
            policy: policy(PolicyAction::Ask),
            approvals: true,
            approval: Some(approval.clone()),
            ..build(vec![Arc::new(Echo)], None)
        });
        assert_eq!(ask.ask(brokered("echo")).await.unwrap().content, "hi");
        assert_eq!(*lock(&seen), [(true, false)], "human-only card, double refused");
    }

    /// A human-only tool with a card preview: it keeps what it showed under the call's key and
    /// publishes exactly that.
    #[derive(Default)]
    struct Previewing {
        previews: std::sync::atomic::AtomicUsize,
        shown: Mutex<HashMap<(SessionId, String), String>>,
        published: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Tool for Arc<Previewing> {
        const NAME: &'static str = "publish";
        const RISK: Risk = Risk::Destructive;
        const HUMAN_ONLY: bool = true;
        const APPROVAL_PREVIEW: bool = true;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn approval_preview(
            &self,
            args: Self::Args,
            cx: PreviewCx,
        ) -> anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>> {
            let n = self
                .previews
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::ensure!(args.get("refuse").is_none(), "switched off");
            let url = format!("https://n{n}.tun.zip/");
            lock(&self.shown).insert((cx.session, cx.call_id), url.clone());
            let mut m = serde_json::Map::new();
            m.insert("url".into(), url.into());
            Ok(Some(m))
        }
        async fn call(&self, args: Self::Args, cx: ToolCx) -> anyhow::Result<ToolOutput> {
            assert!(
                args.get("url").is_none(),
                "the card's keys reached the call"
            );
            let url = lock(&self.shown)
                .remove(&(cx.session, cx.call_id))
                .ok_or_else(|| anyhow::anyhow!("no preview"))?;
            lock(&self.published).push(url.clone());
            Ok(ToolOutput::ok(url))
        }
    }

    /// Answers every card with one decision and records the card's args.
    #[derive(kameo::Actor)]
    struct Answer(
        ActorRef<Approval>,
        Decision,
        Arc<Mutex<Vec<serde_json::Value>>>,
    );

    impl Message<crate::approval::ApprovalRequested> for Answer {
        type Reply = ();
        async fn handle(
            &mut self,
            m: crate::approval::ApprovalRequested,
            _: &mut Context<Self, ()>,
        ) {
            lock(&self.2).push(m.args);
            let _ = self
                .0
                .ask(crate::approval::Resolve {
                    id: m.id,
                    decision: self.1,
                })
                .await;
        }
    }

    /// 210 §7.3: the registry awaits the tool's preview once before the card, the card carries
    /// its keys, `call` gets the model's args and acts on the previewed value; a denied card
    /// never runs the call; a failed preview raises no card.
    #[tokio::test]
    async fn approval_preview_fills_the_card_and_binds_the_call() {
        for decision in [Decision::Allow, Decision::Deny] {
            let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
            let approval = Approval::spawn(Approval::new(rt.clone(), Duration::from_secs(5)));
            let tool = Arc::new(Previewing::default());
            let reg = ToolRegistry::spawn(ToolRegistry {
                approval: Some(approval.clone()),
                ..build(vec![Arc::new(tool.clone())], None)
            });
            let cards = Arc::<Mutex<Vec<serde_json::Value>>>::default();
            let answer = Answer::spawn(Answer(approval.clone(), decision, cards.clone()));
            rt.subscribe::<Answer, crate::approval::ApprovalRequested>(&answer)
                .await;
            let session = SessionId::new();
            let mut c = call("publish", session);
            c.args = serde_json::json!({"source": 8080});
            let out = reg.ask(Invoke(c)).await.unwrap();
            assert_eq!(tool.previews.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(
                *lock(&cards),
                [serde_json::json!({"source": 8080, "url": "https://n0.tun.zip/"})]
            );
            if decision == Decision::Allow {
                assert_eq!(out.content, "https://n0.tun.zip/", "{out:?}");
                assert_eq!(*lock(&tool.published), ["https://n0.tun.zip/"]);
            } else {
                assert_eq!(out.content, "publish: denied by user");
                assert!(lock(&tool.published).is_empty());
            }

            let mut c = call("publish", session);
            c.args = serde_json::json!({"refuse": true});
            let out = reg.ask(Invoke(c)).await.unwrap();
            assert_eq!(out.content, "publish: switched off");
            assert_eq!(lock(&cards).len(), 1, "a refused preview raised a card");
        }
    }

    /// 210 §7.2: policy `deny` still denies a human-only tool; an app press never answers
    /// its `Ask`; a session bound to a non-owner thread, and its subagents, are refused.
    #[tokio::test]
    async fn human_only_refuses_deny_app_press_and_non_owner_threads() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let deny = Policy {
            read_only: PolicyAction::Allow,
            mutating: PolicyAction::Allow,
            destructive: PolicyAction::Deny,
        };
        let reg = registry_with(vec![Arc::new(Publish(runs.clone()))], None, deny, None);
        let session = SessionId::new();
        let out = reg.ask(Invoke(call("publish", session))).await.unwrap();
        assert_eq!(out.content, "publish: denied by policy");

        let reg = registry(vec![Arc::new(Publish(runs.clone()))], None);
        let identity = bound(&reg, session, "publish").await;
        let pressed = reg
            .ask(AppInvoke {
                call: call("publish", session),
                activation: press("1"),
                expected_identity: identity,
            })
            .await
            .unwrap();
        assert!(
            matches!(&pressed, AppInvoked::Unavailable(why) if why.contains("only the owner")),
            "{pressed:?}"
        );

        let (thread, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent: thread,
        })
        .await
        .unwrap();
        reg.tell(SetNonOwnerThread {
            session: thread,
            on: true,
        })
        .await
        .unwrap();
        for s in [thread, child] {
            let out = reg.ask(Invoke(call("publish", s))).await.unwrap();
            assert!(
                out.is_error && out.content.contains("not the owner's"),
                "{out:?}"
            );
        }
        reg.tell(SetNonOwnerThread {
            session: thread,
            on: false,
        })
        .await
        .unwrap();
        // Lifted: back to asking, and no approval actor runs here.
        let out = reg.ask(Invoke(call("publish", thread))).await.unwrap();
        assert!(out.content.contains("approval required"), "{out:?}");
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// What [`Human`] saw: per card, `(always_ask, double's ResolveBy accepted)`.
    type Cards = Arc<Mutex<Vec<(bool, bool)>>>;

    /// Bus subscriber: tries every card as the double first, then answers it as a human
    /// with its decision when the double was refused.
    #[derive(kameo::Actor)]
    struct Human(ActorRef<Approval>, Decision, Cards);

    impl Message<crate::approval::ApprovalRequested> for Human {
        type Reply = ();
        async fn handle(
            &mut self,
            m: crate::approval::ApprovalRequested,
            _: &mut Context<Self, ()>,
        ) {
            let double = self
                .0
                .ask(crate::approval::ResolveBy {
                    id: m.id,
                    decision: Decision::Allow,
                    by: crate::journal::Answerer::Double,
                })
                .await
                .unwrap();
            lock(&self.2).push((m.always_ask, double));
            if !double {
                let _ = self
                    .0
                    .ask(crate::approval::Resolve {
                        id: m.id,
                        decision: self.1,
                    })
                    .await;
            }
        }
    }

    const ALL_ALLOW: Policy = Policy {
        read_only: PolicyAction::Allow,
        mutating: PolicyAction::Allow,
        destructive: PolicyAction::Allow,
    };

    /// A registry with every policy `allow` and `tools.approvals: false`, an approval actor
    /// and a [`Human`] answering `decision` on the bus.
    async fn always_ask_registry(
        tools: Vec<DynTool>,
        global: &[&str],
        config_path: PathBuf,
        decision: Decision,
    ) -> (ActorRef<ToolRegistry>, Cards) {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let approval = Approval::spawn(Approval::new(rt.clone(), Duration::from_secs(5)));
        let reg = ToolRegistry::spawn(ToolRegistry {
            policy: ALL_ALLOW,
            approvals: false,
            approval: Some(approval.clone()),
            always_ask: global.iter().map(|s| (*s).to_owned()).collect(),
            config_path,
            ..build(tools, None)
        });
        let cards: Cards = Arc::default();
        let human = Human::spawn(Human(approval, decision, cards.clone()));
        rt.subscribe::<Human, crate::approval::ApprovalRequested>(&human)
            .await;
        (reg, cards)
    }

    async fn inspect_action(
        reg: &ActorRef<ToolRegistry>,
        session: SessionId,
        name: &str,
    ) -> PolicyAction {
        reg.ask(InspectTool {
            session,
            name: name.into(),
            args: None,
        })
        .await
        .unwrap()
        .action
    }

    /// 219 §7 (a), (g): a harness-wide entry asks with approvals off and every policy
    /// `allow`; the card says `always_ask`, the double's answer is refused and the human's
    /// runs it. An unmarked tool under the same switches never asks.
    #[tokio::test]
    async fn always_ask_global_entry_asks_with_approvals_off() {
        let (reg, cards) = always_ask_registry(
            vec![Arc::new(Echo), Arc::new(Touch)],
            &["echo"],
            PathBuf::new(),
            Decision::Allow,
        )
        .await;
        let session = SessionId::new();
        let out = reg.ask(Invoke(call("touch", session))).await.unwrap();
        assert!(!out.is_error, "{out:?}");
        assert!(lock(&cards).is_empty(), "an unmarked tool asked");
        let out = reg.ask(Invoke(call("echo", session))).await.unwrap();
        assert_eq!(out.content, "hi", "{out:?}");
        assert_eq!(*lock(&cards), [(true, false)]);
    }

    /// 219 §7 (b): a chat's entry asks with the chat's pin off and is inherited by a
    /// subagent; [`GetAlwaysAsk`] reports it as the child's `inherited`; removing it stops
    /// the asking.
    #[tokio::test]
    async fn always_ask_session_entry_is_inherited() {
        let (reg, cards) =
            always_ask_registry(vec![Arc::new(Echo)], &[], PathBuf::new(), Decision::Deny).await;
        let (parent, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        reg.ask(SetApprovals {
            session: Some(parent),
            enabled: false,
            clear: false,
        })
        .await
        .unwrap();
        let state = reg
            .ask(SetAlwaysAsk {
                session: Some(parent),
                tool: "echo".into(),
                on: true,
            })
            .await
            .unwrap();
        assert_eq!(state.session, ["echo"]);
        for s in [parent, child] {
            let out = reg.ask(Invoke(call("echo", s))).await.unwrap();
            assert_eq!(out.content, "echo: denied by user", "{out:?}");
        }
        assert_eq!(*lock(&cards), [(true, false), (true, false)]);
        let state = reg
            .ask(GetAlwaysAsk {
                session: Some(child),
            })
            .await
            .unwrap();
        assert_eq!(
            state,
            AlwaysAskState {
                global: vec![],
                session: vec![],
                inherited: vec!["echo".into()],
            }
        );
        reg.ask(SetAlwaysAsk {
            session: Some(parent),
            tool: "echo".into(),
            on: false,
        })
        .await
        .unwrap();
        let out = reg.ask(Invoke(call("echo", child))).await.unwrap();
        assert_eq!(out.content, "hi", "{out:?}");
        assert_eq!(lock(&cards).len(), 2);
    }

    /// 219 §7 (c), (d): an `mcp:<server>` entry marks the server's tools, and only them; a
    /// cap's `Deny` still wins over always ask.
    #[tokio::test]
    async fn always_ask_matches_mcp_server_and_cap_still_denies() {
        let reg = ToolRegistry::spawn(ToolRegistry {
            policy: ALL_ALLOW,
            approvals: false,
            always_ask: ["mcp:files".to_owned(), "touch".to_owned()].into(),
            ..build(
                vec![
                    Arc::new(FilesMcp("search", Risk::ReadOnly)),
                    Arc::new(Echo),
                    Arc::new(Touch),
                ],
                None,
            )
        });
        let session = SessionId::new();
        assert_eq!(
            inspect_action(&reg, session, "search").await,
            PolicyAction::Ask
        );
        assert_eq!(
            inspect_action(&reg, session, "echo").await,
            PolicyAction::Allow
        );
        assert_eq!(
            inspect_action(&reg, session, "touch").await,
            PolicyAction::Ask
        );
        reg.tell(SetCap {
            session,
            cap: Some(Risk::ReadOnly),
        })
        .await
        .unwrap();
        let out = reg.ask(Invoke(call("touch", session))).await.unwrap();
        assert_eq!(
            out.content, "touch: denied: subagent is capped at read_only",
            "{out:?}"
        );
    }

    /// 219 §7 (e): with no owner in the conversation an always-ask call is denied, with the
    /// contract's reason, for the session and its subagents.
    #[tokio::test]
    async fn always_ask_denies_when_the_owner_is_absent() {
        let (reg, cards) = always_ask_registry(
            vec![Arc::new(Echo)],
            &["echo"],
            PathBuf::new(),
            Decision::Allow,
        )
        .await;
        let (thread, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent: thread,
        })
        .await
        .unwrap();
        reg.tell(SetNonOwnerThread {
            session: thread,
            on: true,
        })
        .await
        .unwrap();
        for s in [thread, child] {
            let out = reg.ask(Invoke(call("echo", s))).await.unwrap();
            assert_eq!(
                out.content,
                "echo: always ask: echo needs the owner's approval and the owner is not in this conversation"
            );
        }
        assert!(lock(&cards).is_empty());
    }

    /// 219 §7 (f): an app press does not approve an always-ask call: an automatic call
    /// needs approval, a pressed one raises a real always-ask card and runs only on the
    /// human's answer.
    #[tokio::test]
    async fn always_ask_app_press_raises_a_card() {
        let (reg, cards) = always_ask_registry(
            vec![Arc::new(Echo)],
            &["echo"],
            PathBuf::new(),
            Decision::Deny,
        )
        .await;
        let session = SessionId::new();
        let identity = bound(&reg, session, "echo").await;
        let auto = reg
            .ask(AppInvoke {
                call: call("echo", session),
                activation: None,
                expected_identity: identity.clone(),
            })
            .await
            .unwrap();
        assert!(matches!(auto, AppInvoked::NeedsApproval), "{auto:?}");
        assert!(lock(&cards).is_empty());
        let pressed = reg
            .ask(AppInvoke {
                call: call("echo", session),
                activation: press("1"),
                expected_identity: identity,
            })
            .await
            .unwrap();
        assert!(
            matches!(&pressed, AppInvoked::Completed(out) if out.content == "echo: denied by user"),
            "{pressed:?}"
        );
        assert_eq!(*lock(&cards), [(true, false)]);
    }

    /// 219 §7 (h): loop-local names and an empty name are refused and change nothing; a
    /// harness-wide entry persists to `tools.always_ask` and a restart reads it back.
    #[tokio::test]
    async fn always_ask_rejects_loop_local_and_persists_global() {
        let dir = scratch_dir();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "# keep me\ntools:\n  approvals: false\n").unwrap();
        let reg = ToolRegistry::spawn(ToolRegistry {
            config_path: path.clone(),
            ..build(vec![Arc::new(Echo)], None)
        });
        for name in LOOP_LOCAL_TOOLS {
            let err = reg
                .ask(SetAlwaysAsk {
                    session: None,
                    tool: (*name).into(),
                    on: true,
                })
                .await
                .unwrap_err();
            let kameo::error::SendError::HandlerError(err) = err else {
                panic!("{err:?}");
            };
            assert_eq!(
                err.to_string(),
                format!(
                    "always_ask: {name} runs inside the agent loop and is never gated; mark the runtime functions it calls instead"
                )
            );
        }
        let err = reg
            .ask(SetAlwaysAsk {
                session: None,
                tool: " ".into(),
                on: true,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            kameo::error::SendError::HandlerError(AlwaysAskError::Empty)
        ));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# keep me\ntools:\n  approvals: false\n"
        );
        for tool in ["mcp:files", "echo"] {
            reg.ask(SetAlwaysAsk {
                session: None,
                tool: tool.into(),
                on: true,
            })
            .await
            .unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"), "{text}");
        let rt = Runtime::new(
            figment::Figment::new()
                .merge(<figment::providers::Yaml as figment::providers::Format>::file(&path)),
            dir.clone(),
        );
        let cfg = config(&rt).unwrap();
        assert_eq!(cfg.always_ask, ["echo", "mcp:files"]);
        assert!(!cfg.approvals);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 219 §7 (i): a built-in file tool writing `<data_dir>/config.yaml` or
    /// `<data_dir>/mcp/config.yaml` asks with an always-ask card even with approvals off
    /// and nothing marked; another file in the data dir does not.
    #[tokio::test]
    async fn always_ask_guards_config_file_writes() {
        let dir = scratch_dir();
        let config_path = dir.join("config.yaml");
        std::fs::write(&config_path, "tools: {}\n").unwrap();
        let (reg, cards) = always_ask_registry(
            vec![Arc::new(fs::WriteFile), Arc::new(crate::text::MoveText)],
            &[],
            config_path.clone(),
            Decision::Deny,
        )
        .await;
        let session = SessionId::new();
        let write = |path: PathBuf| {
            app_call(
                "write_file",
                session,
                serde_json::json!({"path": path, "content": "x"}),
            )
        };
        let out = reg.ask(Invoke(write(config_path.clone()))).await.unwrap();
        assert_eq!(out.content, "write_file: denied by user", "{out:?}");
        assert_eq!(*lock(&cards), [(true, false)]);
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            "tools: {}\n"
        );
        // A `..` spelling of the same file is caught too.
        let sneaky = dir.join("mcp").join("..").join("config.yaml");
        std::fs::create_dir_all(dir.join("mcp")).unwrap();
        let out = reg.ask(Invoke(write(sneaky))).await.unwrap();
        assert_eq!(out.content, "write_file: denied by user", "{out:?}");
        let mcp = reg
            .ask(InspectTool {
                session,
                name: "write_file".into(),
                args: Some(serde_json::json!({"path": dir.join("mcp/config.yaml"), "content": ""})),
            })
            .await
            .unwrap();
        assert_eq!(mcp.action, PolicyAction::Ask);
        let moved = reg
            .ask(InspectTool {
                session,
                name: "move_text".into(),
                args: Some(serde_json::json!({
                    "src": format!("{}#L1-L1@abc", config_path.display()),
                    "at": format!("{}#end", dir.join("other.txt").display()),
                })),
            })
            .await
            .unwrap();
        assert_eq!(moved.action, PolicyAction::Ask);
        let out = reg.ask(Invoke(write(dir.join("other.txt")))).await.unwrap();
        assert!(!out.is_error, "{out:?}");
        assert_eq!(lock(&cards).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A native `add_mcp_server` stand-in (Mutating, runs nothing).
    struct AddMcp;

    #[async_trait::async_trait]
    impl Tool for AddMcp {
        const NAME: &'static str = "add_mcp_server";
        const RISK: Risk = Risk::Mutating;
        type Args = serde_json::Value;
        fn description(&self) -> String {
            String::new()
        }
        async fn call(&self, _: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok("added"))
        }
    }

    /// 219 §2.4 (M2): while any always-ask entry applies to a session, re-registering an MCP
    /// server (which could shed `mcp:<server>` marks) is always ask there; with none it is not.
    #[tokio::test]
    async fn mcp_admin_tools_ask_while_any_entry_applies() {
        let (reg, _cards) =
            always_ask_registry(vec![Arc::new(AddMcp)], &[], PathBuf::new(), Decision::Deny).await;
        let (parent, child, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        assert_eq!(
            inspect_action(&reg, child, "add_mcp_server").await,
            PolicyAction::Allow
        );
        reg.ask(SetAlwaysAsk {
            session: Some(parent),
            tool: "mcp:files".into(),
            on: true,
        })
        .await
        .unwrap();
        for s in [parent, child] {
            assert_eq!(
                inspect_action(&reg, s, "add_mcp_server").await,
                PolicyAction::Ask
            );
        }
        assert_eq!(
            inspect_action(&reg, other, "add_mcp_server").await,
            PolicyAction::Allow,
            "no entry applies to an unrelated chat"
        );
        let out = reg
            .ask(Invoke(call("add_mcp_server", child)))
            .await
            .unwrap();
        assert_eq!(out.content, "add_mcp_server: denied by user", "{out:?}");
    }

    /// 219 §3.2 (m3): a session whose stored list could not be read asks for every
    /// `Mutating`-or-above call in it and its subagents, refuses to overwrite its list, and a
    /// successful restore lifts it.
    #[tokio::test]
    async fn unreadable_always_ask_row_fails_closed_until_restored() {
        let (reg, _cards) = always_ask_registry(
            vec![Arc::new(Echo), Arc::new(Touch)],
            &[],
            PathBuf::new(),
            Decision::Deny,
        )
        .await;
        let (parent, child) = (SessionId::new(), SessionId::new());
        reg.tell(SetParent {
            session: child,
            parent,
        })
        .await
        .unwrap();
        reg.ask(AlwaysAskUnreadable { session: parent })
            .await
            .unwrap();
        for s in [parent, child] {
            assert_eq!(inspect_action(&reg, s, "touch").await, PolicyAction::Ask);
            assert_eq!(inspect_action(&reg, s, "echo").await, PolicyAction::Allow);
        }
        let err = reg
            .ask(SetAlwaysAsk {
                session: Some(parent),
                tool: "touch".into(),
                on: true,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                kameo::error::SendError::HandlerError(AlwaysAskError::NotWritten(_))
            ),
            "{err:?}"
        );
        reg.ask(RestoreAlwaysAsk {
            session: parent,
            names: vec![],
        })
        .await
        .unwrap();
        assert_eq!(
            inspect_action(&reg, child, "touch").await,
            PolicyAction::Allow
        );
    }

    /// A journal whose `sessions.always_ask` write fails.
    struct FailAlwaysAsk;

    impl Journal for FailAlwaysAsk {
        fn call_started(&self, _: SessionId, _: &str) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn call_finished(
            &self,
            _: SessionId,
            _: &str,
            _: &ToolOutput,
        ) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn question(&self, _: QuestionRow) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn wait(
            &self,
            _: Option<crate::WaitRow>,
            _: SessionId,
            _: &str,
        ) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn job(&self, _: crate::JobRow) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn pty_screen(&self, _: crate::PtyScreenRow) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn always_ask(
            &self,
            _: SessionId,
            _: Vec<String>,
        ) -> futures::future::BoxFuture<'_, Result<(), String>> {
            Box::pin(async { Err("disk full".to_owned()) })
        }
    }

    /// 219 §3.2 (m4): a chat's entry is written before it is applied: a failed write
    /// returns `NotWritten` and changes nothing in memory.
    #[tokio::test]
    async fn session_always_ask_not_applied_when_not_written() {
        let reg = ToolRegistry::spawn(ToolRegistry {
            journal: Arc::new(FailAlwaysAsk),
            ..build(vec![Arc::new(Echo)], None)
        });
        let session = SessionId::new();
        let err = reg
            .ask(SetAlwaysAsk {
                session: Some(session),
                tool: "echo".into(),
                on: true,
            })
            .await
            .unwrap_err();
        let kameo::error::SendError::HandlerError(err) = err else {
            panic!("{err:?}");
        };
        assert_eq!(
            err.to_string(),
            "always_ask: sessions.always_ask not written: disk full"
        );
        let state = reg
            .ask(GetAlwaysAsk {
                session: Some(session),
            })
            .await
            .unwrap();
        assert!(state.session.is_empty(), "{state:?}");
    }
}
