//! The JSON-RPC surface. Every method is a lazy registry lookup plus one
//! `ask`; the subscription drains a `broadcast::Receiver` fed by
//! [`crate::bridge`].

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;

use ::search::SearchCategory;
use a2a::{A2A_ACTOR, A2aActor, A2aStatus};
use agent::todo::TodoSnapshot;
use agent::{
    AddAcp, AddProvider, AddSkill, AddSkillRepo, AgentConfig, AgentInfo, Agents, AnswerPeerRequest,
    Cancel, CommunicationState, Compact, CompactStyle, ContextItems, ContextRemoval,
    ContextSnapshot, ContextTarget, DeleteSession, EditTurn, ForkSession, GetCommunication,
    GetCompactStyle, GetMaxSalvages, GetMaxToolRounds, GetPeerMessage, GetStreamIdle,
    GetEscalateModel, GetSubagentModel, GetSubagentTimeout, GetTitleModel, GetTodos, GetVisionModel, Harvest, ListAcp, ListAgents,
    EscalateModel, SetEscalateModel,
    ListLogins, ListModels, ListPeerMessages, ListPeers, ListProviders, ListQueued, ListRegistry,
    ListSessions, ListSkills, LoadSession, LoadSkill, Loaded, Login, LoginInfo, Logout, ModelInfo,
    NewRootSession, PeerMessagePage, PeerPage, PeerReceipt, PeerRecord, PeerScope, PinSession,
    ProviderInfo, QueuedInput, Redo, Regenerate, RemoveAcp, RemoveContext, RemoveProvider,
    RemoveSkill, RemoveSkillRepo, RenameSession, SendInput, SessionSummary, SetCommunication,
    SetCompactStyle, SetEffort, SetMaxSalvages, SetMaxToolRounds, SetModel, SetPeerScope,
    SetStreamIdle, SetSubagentModel, SetSubagentTimeout, SetTitleModel, SetVisionModel, ShowPrompt, SkillBody,
    SkillMeta, Steer, SyncSkills, Undo, UseSkill, Withdraw,
};
use agent::{
    Authorize, Authorized, CloseResult, CreateSession, CreateSessionOptions, LeaveSession,
    PrivateTrees, ResumePrivate, SessionCost, SessionCostQuery, SessionOpen, SetSessionProject,
};
use agent::{AwsProfiles, ListAwsProfiles, SetAwsProfile};
use app::{AppDoc, AppToolResult, Apps, CallArgs, CallStatusOf, CallTool, CloseApp, GetApp};
use artifact::search::ArtifactMatchContext;
use base64::Engine as _;
use browser::{BROWSER_ACTOR, Browsers, Screenshot};
use computer::{COMPUTER_ACTOR, Computer, Permissions};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::types::ErrorObjectOwned;
use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::error::SendError;
use kameo::message::Message;
use kameo::reply::Reply;
use knowledge::search::HistoricalConcept;
use knowledge::{
    Change, Concept, Delete, Get, HarvestResult, History, HistoryOf, Hit, KNOWLEDGE_ACTOR,
    KnowledgeActor, List, Meta, Put, PutConcept, Restore, Revision, RevisionOf, Search, Source,
    SyncNow, Write,
};
use link::{
    CancelPair, Enable, LINK_ACTOR, LinkActor, LinkConfig, LinkStatus, Pair, Pairing, Rename,
    RenameDevice, Revoke,
};
use parking_lot::RwLock;
use runtime::{InputId, Runtime, SessionId};
use serde::{Deserialize, Serialize};
use signal::{Allow, Link, RemoveAllowed, SIGNAL_ACTOR, Signal, SignalStatus, Status, Unlink};
use slack::{SLACK_ACTOR, Slack, SlackStatus};
use tokio::sync::broadcast;
use tools::git::SyncResult;
use tools::{
    AddRepo, AddServer, Answer, Approval, ApprovalId, ApprovalsState, AskId, CancelJob, Decision,
    Gate, GetApprovals, GetPtyIdle, JOBS_ACTOR, JobId, JobView, Jobs, ListJobs, ListServers,
    ListTools, Mcp, McpLogin, McpLogout, McpProgress, McpServer, Progress, RemoveServer, RepoInfo,
    Resolve, SetApprovals, SetGate, SetHidden, SetPtyIdle, SyncMcp, ToolRegistry, ToolSpec,
};
use tools::{
    Key, ListTerminals, PtyAttach, PtyChunk, PtyInput, PtyOut, PtyResize, PtyScreen,
    PtyScreenReply, PtyViewport, Retention, TerminalSummary,
};
use uuid::Uuid;
use voice::{SpeechId, VoiceStatus};

use crate::bridge::Event;
use crate::search::SearchResponse;

/// `ask` failed (handler error, actor stopped mid-call).
const ASK_FAILED: i32 = -32000;
/// The named actor is not registered.
const NOT_RUNNING: i32 = -32001;
/// The call reached the method without an authenticated connection behind it.
const UNAUTHORIZED: i32 = -32002;

/// Reply to `info`: what a client needs before its first session (25 §7);
/// the in-process TUI reads the same things off the [`Runtime`] directly.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Info {
    /// Harness version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// Host OS (`std::env::consts::OS`: `linux`, `macos`, `windows`, …); a
    /// parent shows it beside a node's name (32 §6.2).
    pub platform: String,
    /// `<data_dir>`: config, sqlite, logs, attachments.
    pub data_dir: String,
    /// `agent.model`: the model new sessions start on.
    pub default_model: String,
    /// The global approval switch (`tools.approvals`).
    pub approvals: ApprovalsState,
    /// `agent.compact_style`.
    pub compact_style: CompactStyle,
    /// `agent.subagents.max_runtime_secs`.
    pub subagent_timeout_secs: u64,
    /// `agent.max_tool_rounds`: tool-call rounds a turn may run before the loop lands.
    pub max_tool_rounds: u32,
    /// `agent.stream_idle_secs`: seconds without a stream event before a model call is abandoned.
    pub stream_idle_secs: u64,
    /// `agent.max_salvages`: interrupted-reply recoveries per turn before it stops with an error.
    pub max_salvages: u32,
    /// `tools.pty_idle_secs`: quiet-screen seconds before a PTY job wakes the model with `<job_idle>` (61); `0` = never.
    pub pty_idle_secs: u64,
    /// Harness display name (`link.name`, else the hostname): what a linked
    /// device shows in its harness switcher (31 §6.4).
    pub name: String,
    /// The global communication switch (50 §10): `session: None`, the
    /// authoritative ceiling every per-session state inherits.
    pub communication: CommunicationState,
    /// The app plugin is running (`app.enabled`, 46 §9): `get_app`/`app_*`
    /// answer and the `app` tool exists. `false` leaves app artifacts
    /// readable as source only. Absent from an older harness → `false`.
    #[serde(default)]
    pub app_enabled: bool,
}

/// The wire API. `#[rpc(server)]` generates `ApiServer` (+ `into_rpc()`).
#[rpc(server)]
pub trait Api {
    /// Version, data dir, default model, approvals and compaction style in one
    /// call: the client's startup snapshot.
    #[method(name = "info")]
    async fn info(&self) -> RpcResult<Info>;
    /// Create an idle `owner`-scope root (50 §2.3; the caller authenticated
    /// as the owner) so per-session state can be set before the first
    /// message; `send_input` to it afterwards.
    #[method(name = "new_session")]
    async fn new_session(&self) -> RpcResult<SessionId>;
    /// Start a turn; `session: None` creates an `owner`-scope root first.
    /// `files` are image files (17 §2) attached to the message. Returns once queued.
    #[method(name = "send_input", with_extensions)]
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<SessionId>;
    /// Queue a user message after the current tool batch; returns the id (minted
    /// when omitted). `files` are image files (17 §2).
    #[method(name = "steer", with_extensions)]
    async fn steer(
        &self,
        session: SessionId,
        text: String,
        id: Option<InputId>,
        files: Vec<PathBuf>,
    ) -> RpcResult<InputId>;
    /// Pull a queued message back out; returns its text.
    #[method(name = "withdraw", with_extensions)]
    async fn withdraw(&self, session: SessionId, id: InputId) -> RpcResult<String>;
    /// Messages waiting for the current tool batch to end.
    #[method(name = "list_queued", with_extensions)]
    async fn list_queued(&self, session: SessionId) -> RpcResult<Vec<QueuedInput>>;
    /// Abort the in-flight turn.
    #[method(name = "cancel", with_extensions)]
    async fn cancel(&self, session: SessionId) -> RpcResult<()>;
    /// Background jobs of a session (14 §2): id, state, seconds, log path, tail.
    #[method(name = "list_jobs", with_extensions)]
    async fn list_jobs(&self, session: SessionId) -> RpcResult<Vec<JobView>>;
    /// Kill a running job; `Err` when unknown or already finished.
    #[method(name = "cancel_job", with_extensions)]
    async fn cancel_job(&self, session: SessionId, id: JobId) -> RpcResult<()>;
    /// PTY jobs of `session` (and, with `descendants: true`, its subtree)
    /// whose terminal is still retained (51 §7.3).
    #[method(name = "list_terminals", with_extensions)]
    async fn list_terminals(
        &self,
        session: SessionId,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<TerminalSummary>>;
    /// One page of a PTY job's retained terminal (51 §7): `Unchanged` when
    /// `if_revision` still matches, `Unavailable` when the job's terminal is
    /// gone. An error `terminal busy` means every render slot is taken; retry.
    #[method(name = "pty_screen", with_extensions)]
    async fn pty_screen(
        &self,
        session: SessionId,
        job: JobId,
        viewport: PtyViewport,
        if_revision: Option<u64>,
    ) -> RpcResult<PtyScreenReply>;
    /// A human types into a running PTY job of `session` (55 §2.1): `text`
    /// verbatim, then `keys`, encoded for the program's current modes. Returns
    /// once written; the viewer keeps polling `pty_screen`. Errors when the
    /// job is unknown, finished, or the write fails.
    #[method(name = "pty_input", with_extensions)]
    async fn pty_input(
        &self,
        session: SessionId,
        job: JobId,
        text: Option<String>,
        keys: Option<Vec<Key>>,
    ) -> RpcResult<()>;
    /// Resize a running PTY job's grid (the program gets SIGWINCH). Rows
    /// 2–200, columns 10–400. Errors when the job is unknown or finished.
    #[method(name = "pty_resize", with_extensions)]
    async fn pty_resize(&self, session: SessionId, job: JobId, rows: u16, cols: u16) -> RpcResult<()>;
    /// Attach a terminal emulator to a PTY job: first a `reset` item with
    /// the retained buffer as escape sequences (base64), then every byte the
    /// program writes as `data` items, in order, and `exit` once it ends.
    /// `rows`/`cols` resize the pty before the snapshot. A client that falls
    /// behind gets a fresh `reset` instead of a gap.
    #[subscription(name = "subscribe_pty", unsubscribe = "unsubscribe_pty", item = PtyChunk, with_extensions)]
    async fn subscribe_pty(
        &self,
        session: SessionId,
        job: JobId,
        rows: Option<u16>,
        cols: Option<u16>,
    ) -> SubscriptionResult;
    /// Current page of `session`'s embedded browser as a base64 PNG for a live
    /// view (23 §3.2); `None` when the session has no live isolate or no open
    /// page. Polling never spawns an isolate.
    #[method(name = "browser_screenshot", with_extensions)]
    async fn browser_screenshot(&self, session: SessionId) -> RpcResult<Option<String>>;
    /// Computer-control permissions and attached displays (design doc 27 §6);
    /// errors with "actor not running" when `computer.enabled` is false.
    #[method(name = "computer_permissions")]
    async fn computer_permissions(&self) -> RpcResult<computer::PermissionStatus>;
    /// Fold the conversation now (design doc 07 §8.6 / 11 §4.2); returns the
    /// new epoch. `style: None` uses the configured default. Refused while a
    /// turn is running.
    #[method(name = "compact", with_extensions)]
    async fn compact(&self, session: SessionId, style: Option<CompactStyle>) -> RpcResult<u32>;
    /// Hide the last turn from the frame; returns its first line.
    #[method(name = "undo", with_extensions)]
    async fn undo(&self, session: SessionId) -> RpcResult<String>;
    /// Restore the last undone turn; returns its first line.
    #[method(name = "redo", with_extensions)]
    async fn redo(&self, session: SessionId) -> RpcResult<String>;
    /// Inventory of the active context (design doc 48 §3.2): revision,
    /// editability, and every removable message/binary/head with its
    /// protocol companions. No bodies. Native sessions only.
    #[method(name = "context_items", with_extensions)]
    async fn context_items(&self, session: SessionId) -> RpcResult<ContextSnapshot>;
    /// Remove `targets` from the active context at `expected_revision`
    /// (design doc 48 §3.2). Atomic: stale revision, unknown targets or an
    /// incomplete protocol group refuse and write nothing. Refused while a
    /// turn is running; a `ContextChanged` follows on the stream.
    #[method(name = "remove_context", with_extensions)]
    async fn remove_context(
        &self,
        session: SessionId,
        expected_revision: String,
        targets: Vec<ContextTarget>,
    ) -> RpcResult<ContextRemoval>;
    /// Regenerate the reply to the turn opened at `turn` (a `HistoryEntry`
    /// `turn` handle): drop it and everything after, replay the stored opener
    /// (design doc 29 §3). Refused while a turn is running.
    #[method(name = "regenerate", with_extensions)]
    async fn regenerate(&self, session: SessionId, turn: i64) -> RpcResult<()>;
    /// Replace the turn opened at `turn` with `text` (+ `files`): drop it and
    /// everything after, start a turn with the new input (design doc 29 §3).
    /// Refused while a turn is running.
    #[method(name = "edit_turn", with_extensions)]
    async fn edit_turn(
        &self,
        session: SessionId,
        turn: i64,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<()>;
    /// New session with a copy of the conversation through the turn opened at
    /// `through` (all of it when `None`); returns its id (design doc 29 §3).
    /// The source is untouched. Refused for external harness sessions.
    #[method(name = "fork_session", with_extensions)]
    async fn fork_session(
        &self,
        session: SessionId,
        through: Option<i64>,
        reply: Option<u32>,
    ) -> RpcResult<SessionId>;
    /// The live default compaction style (`agent.compact_style`).
    #[method(name = "compact_style")]
    async fn compact_style(&self) -> RpcResult<CompactStyle>;
    /// Persist `agent.compact_style` and update live sessions.
    #[method(name = "set_compact_style")]
    async fn set_compact_style(&self, style: CompactStyle) -> RpcResult<()>;
    /// Persist `agent.subagents.max_runtime_secs` and use it for newly spawned turns.
    #[method(name = "set_subagent_timeout")]
    async fn set_subagent_timeout(&self, seconds: u64) -> RpcResult<()>;
    /// Persist `agent.max_tool_rounds`; new sessions use it.
    #[method(name = "set_max_tool_rounds")]
    async fn set_max_tool_rounds(&self, rounds: u32) -> RpcResult<()>;
    /// Persist `agent.stream_idle_secs`; new sessions use it.
    #[method(name = "set_stream_idle")]
    async fn set_stream_idle(&self, seconds: u64) -> RpcResult<()>;
    /// Persist `agent.max_salvages`; new sessions use it.
    #[method(name = "set_max_salvages")]
    async fn set_max_salvages(&self, count: u32) -> RpcResult<()>;
    /// Persist `tools.pty_idle_secs` (61 §1.6); `start` calls without `idle_secs` use it from then on. `0` = never.
    #[method(name = "set_pty_idle")]
    async fn set_pty_idle(&self, seconds: u64) -> RpcResult<()>;
    /// The live title model (`agent.title_model`), if set.
    #[method(name = "title_model")]
    async fn title_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `agent.title_model`; `None` clears it (titles use the session's model).
    #[method(name = "set_title_model")]
    async fn set_title_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// The live vision fallback model (`agent.vision_model`), if set (52 §3.1).
    #[method(name = "vision_model")]
    async fn vision_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `agent.vision_model`; `None` clears it (no fallback). Rejects
    /// anything but an exact, image-capable catalog entry on a configured
    /// provider; a failed config write is an error and changes nothing.
    #[method(name = "set_vision_model")]
    async fn set_vision_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// The escalation model (`agent.escalate_model`, 57 §0.9: tier 2 of the
    /// local decision oracle) and what a tier-2 call resolves to today.
    #[method(name = "escalate_model")]
    async fn escalate_model(&self) -> RpcResult<EscalateModel>;
    /// Persist `agent.escalate_model`; `None` clears it (inherits the title
    /// model, then the session's model). Rejects an id that is not a catalog
    /// entry; a failed config write is an error and changes nothing.
    #[method(name = "set_escalate_model")]
    async fn set_escalate_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// The live preferred subagent model (`agent.subagents.model`), if set.
    #[method(name = "subagent_model")]
    async fn subagent_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `agent.subagents.model`; `None` clears it (children use `agent.model`).
    /// Children spawned without `model` use it and the system prompt names it.
    #[method(name = "set_subagent_model")]
    async fn set_subagent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// Every session, newest first.
    #[method(name = "list_sessions")]
    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>>;
    /// The subtree under `session` (children, grandchildren, …), pre-order,
    /// each with its live state (19 §2.1).
    #[method(name = "list_agents", with_extensions)]
    async fn list_agents(&self, session: SessionId) -> RpcResult<Vec<AgentInfo>>;
    /// Rendered history of one session plus whether a turn is in flight.
    /// `before`/`limit` page the entries (25 §7): those with `seq < before`,
    /// the last `limit` of them; both omitted = everything. `settled_only`
    /// (48 §5) reads through the idle session actor; while busy the reply
    /// has `settled: false` and `context_editable: false` with a reason.
    #[method(name = "load_session", with_extensions)]
    async fn load_session(
        &self,
        session: SessionId,
        before: Option<i64>,
        limit: Option<u32>,
        settled_only: Option<bool>,
    ) -> RpcResult<Loaded>;
    /// The session's current plan (47 §5.1): the latest committed snapshot,
    /// readable while a turn runs; `TodoChanged` carries later versions.
    /// An unknown session or a failed read is an error, never an empty list.
    #[method(name = "get_todos", with_extensions)]
    async fn get_todos(&self, session: SessionId) -> RpcResult<TodoSnapshot>;
    /// Model-usage summary of `session` as an accounting owner (51 §6.5):
    /// direct, descendants, total, children, coverage and a revision that
    /// `ModelUsageChanged` bumps. `Err` when the session owns no ledger.
    #[method(name = "session_cost", with_extensions)]
    async fn session_cost(&self, session: SessionId) -> RpcResult<SessionCost>;
    /// Set a session's title by hand; `TitleChanged` follows on the stream.
    #[method(name = "rename_session", with_extensions)]
    async fn rename_session(&self, session: SessionId, title: String) -> RpcResult<()>;
    /// Pin or unpin a session; pinned sessions lead `list_sessions`.
    #[method(name = "pin_session", with_extensions)]
    async fn pin_session(&self, session: SessionId, pinned: bool) -> RpcResult<()>;
    /// Delete a session and its subagents for good; a `SessionDeleted` per removed session
    /// follows on the stream. Refused while any of them is mid-turn.
    #[method(name = "delete_session", with_extensions)]
    async fn delete_session(&self, session: SessionId) -> RpcResult<()>;
    /// Open a root with explicit storage (51 §4.1): `retention: durable`
    /// (default) is `new_session` plus `project`; `private` needs `view` and
    /// binds the tree to the calling connection, returning the lease secret
    /// it presents on `resume_private` / `leave_session`. Always returns
    /// `SessionOpen`.
    #[method(name = "create_session", with_extensions)]
    async fn create_session(&self, options: CreateSessionOptions) -> RpcResult<SessionOpen>;
    /// Rebind a private root to this connection within the reconnect window
    /// (51 §4.3): same `view`, the current `lease_secret`; rotates the secret.
    #[method(name = "resume_private", with_extensions)]
    async fn resume_private(
        &self,
        session: SessionId,
        view: String,
        lease_secret: String,
    ) -> RpcResult<SessionOpen>;
    /// Release a root this connection owns (51 §4.3): private → the tree
    /// closes now and `CloseResult` says how far cleanup got; durable →
    /// `revoked: false`, nothing to clean.
    #[method(name = "leave_session", with_extensions)]
    async fn leave_session(
        &self,
        session: SessionId,
        lease_secret: Option<String>,
    ) -> RpcResult<CloseResult>;
    /// Set (or clear with `None`) the project label of `session`'s root
    /// (51 §3); `SessionMetadataChanged` follows on the stream.
    #[method(name = "set_session_project", with_extensions)]
    async fn set_session_project(
        &self,
        session: SessionId,
        project: Option<String>,
    ) -> RpcResult<()>;
    /// Store base64 `bytes` under `session`'s tree (51 §8.1): the durable
    /// attachments dir, or the private scratch root — ownership is resolved
    /// before any byte is written. Same extension / size rules as
    /// `put_attachment` (`file_ext` ∈ png, jpg, …).
    #[method(name = "put_session_attachment", with_extensions)]
    async fn put_session_attachment(
        &self,
        session: SessionId,
        bytes: String,
        file_ext: String,
    ) -> RpcResult<PathBuf>;
    /// Unified human search (design doc 49 §4.2) over sessions, artifacts,
    /// current knowledge and knowledge history: one group per selected category
    /// in fixed order, each `ready`/`indexing`/`unavailable`/`error` on its own.
    /// `categories` omitted → all four, `[]` → none; `limit` per category,
    /// default 10, max 50. Blank query → no groups, no cache work.
    /// Clients re-ask only `indexing` groups (`categories: [that]`) every 500 ms.
    #[method(name = "search")]
    async fn search(
        &self,
        query: String,
        categories: Option<Vec<SearchCategory>>,
        limit: Option<u32>,
    ) -> RpcResult<SearchResponse>;
    /// Retry `search`'s failed owners without matching: the selected owners
    /// (omitted → all four) drop failed/stale search build state so the next
    /// `search` rebuilds; knowledge also re-syncs current concepts with disk.
    /// Owners not running are skipped; any failing reset is the error.
    #[method(name = "search_refresh")]
    async fn search_refresh(&self, categories: Option<Vec<SearchCategory>>) -> RpcResult<()>;
    /// Every registered tool.
    #[method(name = "list_tools")]
    async fn list_tools(&self) -> RpcResult<Vec<ToolSpec>>;
    /// Answer a pending approval; `true` if it was still pending.
    #[method(name = "resolve_approval")]
    async fn resolve_approval(&self, id: ApprovalId, decision: Decision) -> RpcResult<bool>;
    /// Answer a pending `ask_user`: one answer per question, or `None` to
    /// dismiss the set, which also cancels the turn. `true` if it was still pending.
    #[method(name = "answer_ask")]
    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> RpcResult<bool>;
    /// Every catalog model whose provider is configured.
    #[method(name = "list_models")]
    async fn list_models(&self) -> RpcResult<Vec<ModelInfo>>;
    /// AWS SDK-discovered profiles on this harness, plus its inference selection.
    #[method(name = "list_aws_profiles")]
    async fn list_aws_profiles(&self) -> RpcResult<AwsProfiles>;
    /// Select the harness-wide Bedrock inference profile; `None` restores the AWS default chain.
    #[method(name = "set_aws_profile")]
    async fn set_aws_profile(&self, profile: Option<String>) -> RpcResult<AwsProfiles>;
    /// Switch `session` (and the default for new sessions) to `model`.
    #[method(name = "set_model", with_extensions)]
    async fn set_model(&self, session: Option<SessionId>, model: String) -> RpcResult<ModelInfo>;
    /// Set (or clear with `None`) the reasoning effort for `session` from its next turn, and as
    /// the default (`agent.effort`). Values come from the model's catalog entry
    /// (`ModelInfo.efforts`, or an integer token budget within `budget_min..budget_max`);
    /// `EffortChanged` follows on the stream.
    #[method(name = "set_effort", with_extensions)]
    async fn set_effort(&self, session: Option<SessionId>, effort: Option<String>)
    -> RpcResult<()>;
    /// MCP servers, their tools, and their `mcp/config.yaml` risk lines; with
    /// a session, which tools it hid (`set_mcp_hidden`).
    #[method(name = "list_mcp", with_extensions)]
    async fn list_mcp(&self, session: Option<SessionId>) -> RpcResult<Vec<McpServer>>;
    /// MCP connection progress snapshot (`McpProgress`), for a UI attaching after startup.
    #[method(name = "mcp_status")]
    async fn mcp_status(&self) -> RpcResult<McpProgress>;
    /// Set (or clear with `None`) a server or tool risk line
    /// (`read_only | mutating | destructive | off`); persists to `mcp/config.yaml`.
    #[method(name = "set_mcp_gate")]
    async fn set_mcp_gate(
        &self,
        server: String,
        tool: Option<String>,
        gate: Option<Gate>,
    ) -> RpcResult<()>;
    /// Hide (or show again) registered tools for one session and its
    /// subagents, in memory: absent from the model's tool list, `unknown tool`
    /// when called. The persistent form is `set_mcp_gate(.., Some(Off))`.
    #[method(name = "set_mcp_hidden", with_extensions)]
    async fn set_mcp_hidden(
        &self,
        session: SessionId,
        tools: Vec<String>,
        hidden: bool,
    ) -> RpcResult<()>;
    /// Turn approval prompts on/off for `session`, or globally (persisted) with `None`.
    #[method(name = "set_approvals", with_extensions)]
    async fn set_approvals(
        &self,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<ApprovalsState>;
    /// Effective communication state (50 §10): the global switch alone with
    /// `None`, else `session`'s inherited/opted-out view, scope and alias.
    /// Authoritative: render it, never guess it.
    #[method(name = "get_communication", with_extensions)]
    async fn get_communication(&self, session: Option<SessionId>) -> RpcResult<CommunicationState>;
    /// `/communication on|off` for `session` (memory-only opt-out), or the
    /// persisted global ceiling with `None`. A session cannot override an
    /// ancestor or global off; the returned state says what blocks it.
    #[method(name = "set_communication", with_extensions)]
    async fn set_communication(
        &self,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<CommunicationState>;
    /// Owner-only root scope (`owner` | `isolated`) and alias (50 §2.3);
    /// `alias: None` keeps, `Some("")` clears. `Err` for a non-root.
    #[method(name = "set_peer_scope", with_extensions)]
    async fn set_peer_scope(
        &self,
        root: SessionId,
        scope: PeerScope,
        alias: Option<String>,
    ) -> RpcResult<CommunicationState>;
    /// The directory `session` may address (50 §5.1), the same permission
    /// filter the model's `discover_peers` sees; bounded fields only.
    #[method(name = "list_peers", with_extensions)]
    async fn list_peers(
        &self,
        session: SessionId,
        query: Option<String>,
        cursor: Option<String>,
        limit: Option<u16>,
    ) -> RpcResult<PeerPage>;
    /// `session`'s own record of exchange `id` (50 §5.2); `Err` when it is
    /// neither sender nor recipient.
    #[method(name = "peer_message", with_extensions)]
    async fn peer_message(&self, session: SessionId, id: Uuid) -> RpcResult<PeerRecord>;
    /// `session`'s sent and received exchanges, newest first, with the exact
    /// ids a human may resolve now (`awaiting_answer` / `awaiting_reply`);
    /// `pending_only` restricts the rows to those. The durable snapshot a UI
    /// reloads after `PeerMessageChanged` or a reconnect.
    #[method(name = "list_peer_messages", with_extensions)]
    async fn list_peer_messages(
        &self,
        session: SessionId,
        cursor: Option<String>,
        limit: Option<u16>,
        pending_only: bool,
    ) -> RpcResult<PeerMessagePage>;
    /// Resolve pending request `reply_to` on behalf of `session` (50 §5.3):
    /// as its recipient the answer travels to the sender as a peer message
    /// from `session`; as its blocked sender its own wait resolves. Either
    /// way `answered_by = owner:<session>`; never an arbitrary waiter.
    #[method(name = "answer_peer_request", with_extensions)]
    async fn answer_peer_request(
        &self,
        session: SessionId,
        reply_to: Uuid,
        text: String,
    ) -> RpcResult<PeerReceipt>;
    /// The assembled system prompt; without `session` the peers section is omitted.
    #[method(name = "get_prompt", with_extensions)]
    async fn get_prompt(&self, session: Option<SessionId>) -> RpcResult<String>;
    /// Every skill under `<data_dir>/skills`.
    #[method(name = "list_skills")]
    async fn list_skills(&self) -> RpcResult<Vec<SkillMeta>>;
    /// Body (and supporting files) of one skill.
    #[method(name = "load_skill")]
    async fn load_skill(&self, name: String) -> RpcResult<SkillBody>;
    /// Hand a skill to the model as user input: its body wrapped in
    /// `<skill name="…">`, then `args` (design doc 04 §2.4). New turn when
    /// idle, steer when running; `None` session starts one.
    #[method(name = "use_skill", with_extensions)]
    async fn use_skill(
        &self,
        session: Option<SessionId>,
        name: String,
        args: String,
    ) -> RpcResult<SessionId>;
    /// Write `skills/<name>.md`.
    #[method(name = "add_skill")]
    async fn add_skill(
        &self,
        name: String,
        description: String,
        body: String,
        always: bool,
    ) -> RpcResult<SkillMeta>;
    /// Delete a local skill.
    #[method(name = "remove_skill")]
    async fn remove_skill(&self, name: String) -> RpcResult<()>;
    /// Clone a skill repo into `skills/<name>`; returns the skills it holds.
    #[method(name = "add_skill_repo")]
    async fn add_skill_repo(&self, url: String, name: Option<String>) -> RpcResult<Vec<SkillMeta>>;
    /// Delete a cloned skill repo.
    #[method(name = "remove_skill_repo")]
    async fn remove_skill_repo(&self, name: String) -> RpcResult<()>;
    /// `git pull` every skill repo.
    #[method(name = "sync_skills")]
    async fn sync_skills(&self) -> RpcResult<Vec<SyncResult>>;
    /// Search the knowledge bundle (design doc 10 §8); `limit` defaults to 10, max 50.
    #[method(name = "knowledge_search")]
    async fn knowledge_search(&self, query: String, limit: Option<u32>) -> RpcResult<Vec<Hit>>;
    /// One concept verbatim with its links and citers.
    #[method(name = "knowledge_get")]
    async fn knowledge_get(&self, id: String) -> RpcResult<Concept>;
    /// Concept metas sorted by id; `limit` defaults to 50.
    #[method(name = "knowledge_list")]
    async fn knowledge_list(
        &self,
        prefix: Option<String>,
        #[argument(rename = "type")] type_: Option<String>,
        tag: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<Meta>>;
    /// Create or update a concept as `source: user`.
    #[method(name = "knowledge_put")]
    async fn knowledge_put(&self, put: Put) -> RpcResult<Meta>;
    /// Delete a concept (restorable through `knowledge_restore`).
    #[method(name = "knowledge_delete")]
    async fn knowledge_delete(&self, id: String) -> RpcResult<()>;
    /// Create or update a concept from its raw text (frontmatter and body) as
    /// `source: user`; unknown frontmatter keys survive, unlike `knowledge_put`.
    #[method(name = "knowledge_write")]
    async fn knowledge_write(&self, id: String, text: String) -> RpcResult<Meta>;
    /// Reconcile the index with the files on disk now.
    #[method(name = "knowledge_sync")]
    async fn knowledge_sync(&self) -> RpcResult<knowledge::Sync>;
    /// Commits touching `id` (the whole bundle when absent), newest first; `limit` defaults to 20.
    #[method(name = "knowledge_history")]
    async fn knowledge_history(&self, id: Option<String>, limit: Option<u32>)
    -> RpcResult<History>;
    /// `commit` against its first parent (the empty tree for a root commit) as
    /// per-concept unified diffs, including deletions; `id` restricts to one concept.
    #[method(name = "knowledge_revision")]
    async fn knowledge_revision(&self, commit: String, id: Option<String>) -> RpcResult<Revision>;
    /// The full text of concept `id` as committed at `commit` (a full object id
    /// from a `knowledge_history` search hit's `snapshot_commit`); unknown
    /// revision is an error, never the current text (49 §7).
    #[method(name = "knowledge_read_revision")]
    async fn knowledge_read_revision(
        &self,
        id: String,
        commit: String,
    ) -> RpcResult<HistoricalConcept>;
    /// Rewrite `id` from `commit` (default: the commit before its latest change).
    #[method(name = "knowledge_restore")]
    async fn knowledge_restore(&self, id: String, commit: Option<String>) -> RpcResult<Change>;
    /// Harvest `session`'s finished frame now (design doc 10 §5.2). Refused while a turn is running.
    #[method(name = "knowledge_harvest", with_extensions)]
    async fn knowledge_harvest(&self, session: SessionId) -> RpcResult<HarvestResult>;
    /// Write `mcp/<name>.json` and connect; returns the server with its tools.
    #[method(name = "add_mcp_server")]
    async fn add_mcp_server(&self, name: String, entry: serde_json::Value) -> RpcResult<McpServer>;
    /// Remove (or disable) an MCP server entry.
    #[method(name = "remove_mcp_server")]
    async fn remove_mcp_server(&self, name: String) -> RpcResult<()>;
    /// Clone a repo into `mcp/repos/<name>`.
    #[method(name = "add_mcp_repo")]
    async fn add_mcp_repo(&self, url: String, name: Option<String>) -> RpcResult<RepoInfo>;
    /// `git pull` every MCP repo, restarting servers whose repo changed.
    #[method(name = "sync_mcp")]
    async fn sync_mcp(&self) -> RpcResult<Vec<SyncResult>>;
    /// Configured external harnesses (`acp/agents.yaml`); `probe` runs `initialize` on each.
    #[method(name = "list_acp")]
    async fn list_acp(&self, probe: bool) -> RpcResult<Vec<acp::EntryInfo>>;
    /// Install a registry agent as a harness entry (`binary` distributions are downloaded).
    #[method(name = "add_acp")]
    async fn add_acp(&self, id: String) -> RpcResult<acp::EntryInfo>;
    /// Remove a harness entry and its downloaded binary, if any.
    #[method(name = "remove_acp")]
    async fn remove_acp(&self, name: String) -> RpcResult<()>;
    /// The ACP agent registry (cached for a day; offline returns the cache).
    #[method(name = "list_acp_registry")]
    async fn list_acp_registry(&self) -> RpcResult<Vec<acp::registry::RegistryAgent>>;
    /// Providers a user can log in to (design doc 05), with their status.
    #[method(name = "list_logins")]
    async fn list_logins(&self) -> RpcResult<Vec<LoginInfo>>;
    /// Start a provider login; returns the URL to open. Ends with a `LoginFinished` event.
    #[method(name = "login")]
    async fn login(&self, provider: String) -> RpcResult<String>;
    /// Forget a provider login.
    #[method(name = "logout")]
    async fn logout(&self, provider: String) -> RpcResult<()>;
    /// Named OpenAI-compatible gateways (`/provider`).
    #[method(name = "list_providers")]
    async fn list_providers(&self) -> RpcResult<Vec<ProviderInfo>>;
    /// Persist a named OpenAI-compatible gateway (url + key); fetches `/models`.
    #[method(name = "add_provider")]
    async fn add_provider(
        &self,
        name: String,
        base_url: String,
        key: String,
    ) -> RpcResult<ProviderInfo>;
    /// Forget a named OpenAI-compatible gateway.
    #[method(name = "remove_provider")]
    async fn remove_provider(&self, name: String) -> RpcResult<()>;
    /// Start an MCP server login; returns the URL to open. The server reconnects on success (`McpProgress`).
    #[method(name = "mcp_login")]
    async fn mcp_login(&self, server: String) -> RpcResult<String>;
    /// Forget an MCP server login and restart it.
    #[method(name = "mcp_logout")]
    async fn mcp_logout(&self, server: String) -> RpcResult<()>;
    /// Start Signal linking (if not registered); returns the provisioning URL
    /// to render as a QR. `SignalEvent::LinkUrl`/`Linked` follow on the stream.
    #[method(name = "signal_link")]
    async fn signal_link(&self) -> RpcResult<String>;
    /// Signal link/connection state, trust lists and phone threads.
    #[method(name = "signal_status")]
    async fn signal_status(&self) -> RpcResult<SignalStatus>;
    /// Add an E.164 to `signal.allowed` (live and in the config file).
    #[method(name = "signal_allow")]
    async fn signal_allow(&self, number: String) -> RpcResult<()>;
    /// Remove an E.164 from `signal.allowed`.
    #[method(name = "signal_remove_allowed")]
    async fn signal_remove_allowed(&self, number: String) -> RpcResult<()>;
    /// Drop the Signal registration (`presage.db`) and return to linking; threads are kept.
    #[method(name = "signal_unlink")]
    async fn signal_unlink(&self) -> RpcResult<()>;
    /// Slack connection state, trust lists, channels and bound threads.
    #[method(name = "slack_status")]
    async fn slack_status(&self) -> RpcResult<SlackStatus>;
    /// Add a user id (`U…`) to `slack.allowed` (live and in the config file).
    #[method(name = "slack_allow")]
    async fn slack_allow(&self, user: String) -> RpcResult<()>;
    /// Remove a user id from `slack.allowed`.
    #[method(name = "slack_remove_allowed")]
    async fn slack_remove_allowed(&self, user: String) -> RpcResult<()>;
    /// Add a channel id (`C…`) to `slack.channels` (live and in the config file).
    #[method(name = "slack_allow_channel")]
    async fn slack_allow_channel(&self, channel: String) -> RpcResult<()>;
    /// Remove a channel id from `slack.channels`.
    #[method(name = "slack_remove_channel")]
    async fn slack_remove_channel(&self, channel: String) -> RpcResult<()>;
    /// A2A network status (50 §10): listener, configured peers and exports,
    /// and the config/listener/reload error when any. `a2a.enabled: false`
    /// answers the disabled state; enabled without a live actor is an error.
    #[method(name = "a2a_status")]
    async fn a2a_status(&self) -> RpcResult<A2aStatus>;
    /// Re-read `peers.yaml` and the `a2a` section, validate and swap
    /// atomically (50 §6.1); a failed reload keeps the old configuration and
    /// is the `Err`. `A2aChanged` follows on the stream.
    #[method(name = "reload_a2a")]
    async fn reload_a2a(&self) -> RpcResult<A2aStatus>;
    /// Stop the whole process.
    #[method(name = "shutdown")]
    async fn shutdown(&self) -> RpcResult<()>;
    /// Link state: relay, uplink, paired devices, pending pairing (31 §6.4).
    #[method(name = "link_status")]
    async fn link_status(&self) -> RpcResult<LinkStatus>;
    /// Turn the relay uplink on/off (persisted as `link.enabled`); `relay: Some` also rewrites `link.relay`.
    #[method(name = "link_enable")]
    async fn link_enable(&self, enabled: bool, relay: Option<String>) -> RpcResult<()>;
    /// Start (or restart) the single pending pairing: the QR string and its expiry.
    #[method(name = "link_pair")]
    async fn link_pair(&self) -> RpcResult<Pairing>;
    /// Drop the pending pairing.
    #[method(name = "link_cancel_pair")]
    async fn link_cancel_pair(&self) -> RpcResult<()>;
    /// Set the harness display name (persisted as `link.name`; empty → hostname).
    #[method(name = "link_rename")]
    async fn link_rename(&self, name: String) -> RpcResult<()>;
    /// Rename a paired device.
    #[method(name = "link_rename_device")]
    async fn link_rename_device(&self, id: String, name: String) -> RpcResult<()>;
    /// Revoke a paired device: close its conns, delete its row.
    #[method(name = "link_revoke_device")]
    async fn link_revoke_device(&self, id: String) -> RpcResult<()>;
    /// Store base64 `bytes` as `<data_dir>/attachments/<sha256[..16]>.<ext>`
    /// and return the path, for `send_input { files }` or `transcribe`. `ext`
    /// ∈ png, jpg, jpeg, gif, webp, pdf, m4a, mp4, webm, ogg, opus, mp3, wav;
    /// images ≤ 5 MB, PDFs ≤ 32 MB, audio ≤ 16 MB decoded.
    #[method(name = "put_attachment")]
    async fn put_attachment(&self, bytes: String, ext: String) -> RpcResult<PathBuf>;
    /// Read a file the harness produced or stored, base64-encoded. Only
    /// paths under `<root>/{attachments,browser,computer,artifacts}` (after
    /// canonicalization) are served, where `<root>` is `data_dir`, or with
    /// `session` the tree's own root — a private tree's files are served to
    /// its owner only, and never without naming the session (51 §4.2).
    #[method(name = "get_attachment", with_extensions)]
    async fn get_attachment(&self, path: PathBuf, session: Option<SessionId>) -> RpcResult<String>;
    /// Paired nodes with liveness and child counts (32 §6.6).
    #[method(name = "list_nodes")]
    async fn list_nodes(&self) -> RpcResult<Vec<nodes::Node>>;
    /// Pair with a node from its pasted key (31 §4.4 string); `name` overrides
    /// the node's own display name locally (32 §6.6).
    #[method(name = "add_node")]
    async fn add_node(&self, key: String, name: Option<String>) -> RpcResult<nodes::Node>;
    /// Close the node's conns, delete its row and best-effort revoke this
    /// harness on the node (32 §6.6).
    #[method(name = "remove_node")]
    async fn remove_node(&self, id: String) -> RpcResult<()>;
    /// Set a node's local alias (32 §6.6).
    #[method(name = "rename_node")]
    async fn rename_node(&self, id: String, name: String) -> RpcResult<()>;
    /// `link_pair` on the node over the control conn: a QR string a phone
    /// scans to pair with the node directly (32 §6.6, §8.2).
    #[method(name = "invite_node")]
    async fn invite_node(&self, id: String) -> RpcResult<Pairing>;
    /// Every schedule file with its next/last fire and parse state (34 §8).
    #[method(name = "list_schedules")]
    async fn list_schedules(&self) -> RpcResult<Vec<schedule::ScheduleInfo>>;
    /// The self-updater's latest state: `null`, or the version that is on disk
    /// (`applied`) / available but not writable (39 §4.3).
    #[method(name = "update_status")]
    async fn update_status(&self) -> RpcResult<Option<update::UpdateReady>>;
    /// Read `text` (markdown) aloud in `session`'s name, stopping whatever is
    /// speaking first (43 §5). Replies with the utterance's id at once; audio
    /// follows as `SpeechAudio` events on `subscribe_events`.
    #[method(name = "speak", with_extensions)]
    async fn speak(&self, session: SessionId, text: String) -> RpcResult<SpeechId>;
    /// Stop utterance `id`, or whatever is speaking when `null`. Idempotent.
    #[method(name = "stop_speaking")]
    async fn stop_speaking(&self, id: Option<SpeechId>) -> RpcResult<()>;
    /// Model state, configured voice, available voices and the utterance in
    /// progress (43 §5).
    #[method(name = "voice_status")]
    async fn voice_status(&self) -> RpcResult<VoiceStatus>;
    /// Transcribe an audio file stored by `put_attachment` (45 §7.1); `path`
    /// must be under `<data_dir>/attachments`. `NOT_RUNNING` when the voice
    /// plugin is off; a missing STT checkpoint is an error string.
    #[method(name = "transcribe")]
    async fn transcribe(&self, path: PathBuf) -> RpcResult<voice::Transcript>;
    /// Load the streaming dictation model (and the VAD) so Ctrl+D is instant;
    /// never opens the microphone; idempotent — replies with the current
    /// `ModelState`, `Loading` while the load is in flight (54 §0.5, §8).
    /// Frontends call it once after connect.
    #[method(name = "dictation_prewarm", with_extensions)]
    async fn dictation_prewarm(&self) -> RpcResult<voice::ModelState>;
    /// Start streaming text (54 §8); text arrives as `DictationText` events on
    /// `subscribe_events`. `sample_rate: None` opens the daemon's microphone —
    /// local peers only; a relay peer gets "microphone is on the harness host;
    /// pass sample_rate to stream audio". `Some(rate)` (a multiple of 50)
    /// means the caller streams its own mono i16 PCM at `rate` Hz with
    /// `dictation_audio`; the dictation stops when that connection closes.
    /// Other error strings: `already dictating`, `voice is disabled`, a
    /// missing/failed model (with the expected path), a mic open failure.
    #[method(name = "dictation_start", with_extensions)]
    async fn dictation_start(&self, sample_rate: Option<u32>) -> RpcResult<voice::DictationId>;
    /// Feed remote dictation `id`: `pcm` is base64 of little-endian mono i16
    /// samples at the rate given to `dictation_start`, ~100 ms per call.
    /// `Err` when `id` is not a running remote dictation or `pcm` is not
    /// whole i16s.
    #[method(name = "dictation_audio", with_extensions)]
    async fn dictation_audio(&self, id: voice::DictationId, pcm: String) -> RpcResult<()>;
    /// Stop dictation `id`, flushing the open utterance first; `Finished`
    /// follows as an event. Idempotent: an unknown or finished id is `Ok`
    /// (54 §8). Any authenticated peer may stop, including a relay peer.
    #[method(name = "dictation_stop", with_extensions)]
    async fn dictation_stop(&self, id: voice::DictationId) -> RpcResult<()>;
    /// The dictation model's state and the running dictation, if any (54 §8):
    /// how a second frontend learns it is "dictating elsewhere". Open to
    /// relay peers.
    #[method(name = "dictation_status", with_extensions)]
    async fn dictation_status(&self) -> RpcResult<voice::DictationState>;
    /// Processes Core Audio knows about, the ones playing audio first (53, 01 §4.1):
    /// the picker for `meeting_start`. "meetings are disabled" when the plugin is off.
    #[method(name = "meeting_processes")]
    async fn meeting_processes(&self) -> RpcResult<Vec<meeting::ProcessInfo>>;
    /// Start recording `process` (a pid from `meeting_processes`; `null` = all
    /// system audio); `consent` must be `true`. Replies once the devices are
    /// open, after `Started` and `State{Recording}` went out on
    /// `subscribe_events`. Error strings: consent missing, pid not playing,
    /// "a meeting is already recording", a device or permission failure (the
    /// meeting is then `Failed` in `meeting_list`).
    #[method(name = "meeting_start")]
    async fn meeting_start(&self, start: meeting::StartMeeting) -> RpcResult<meeting::Started>;
    /// Pause the recording: devices stay open, the clock stops, a gap is recorded.
    #[method(name = "meeting_pause")]
    async fn meeting_pause(&self) -> RpcResult<()>;
    /// Resume after `meeting_pause`.
    #[method(name = "meeting_resume")]
    async fn meeting_resume(&self) -> RpcResult<()>;
    /// Stop; replies once the last Ogg pages are written. The post pipeline then
    /// runs in the background (`PostProgress` → `ReviewReady`, 04 §2).
    #[method(name = "meeting_stop")]
    async fn meeting_stop(&self) -> RpcResult<()>;
    /// The current meeting's state, ids, elapsed recording time and degradations.
    #[method(name = "meeting_status")]
    async fn meeting_status(&self) -> RpcResult<meeting::Status>;
    /// Recorded meetings, newest first (`limit` default 100).
    #[method(name = "meeting_list")]
    async fn meeting_list(&self, limit: Option<u32>) -> RpcResult<Vec<meeting::MeetingSummary>>;
    /// One meeting's manifest.
    #[method(name = "meeting_get")]
    async fn meeting_get(&self, id: meeting::MeetingId) -> RpcResult<meeting::Manifest>;
    /// Delete a meeting: directory, index row, session. Refused while it records.
    #[method(name = "meeting_delete")]
    async fn meeting_delete(&self, id: meeting::MeetingId) -> RpcResult<()>;
    /// Microphone and audio-capture TCC state as far as it is known without prompting.
    #[method(name = "meeting_permissions")]
    async fn meeting_permissions(&self) -> RpcResult<meeting::Permissions>;
    /// Phase 2 (02 §6). The user tapped a topic button: `TopicRemoved{Viewed}`,
    /// the stored brief as `Brief{source: Tap}`, a `[meeting tapped]` steer.
    #[method(name = "meeting_tap")]
    async fn meeting_tap(&self, topic: meeting::TopicId) -> RpcResult<()>;
    /// The user dismissed a topic button: `TopicRemoved{Dismissed}`, the label barred, a steer.
    #[method(name = "meeting_dismiss")]
    async fn meeting_dismiss(&self, topic: meeting::TopicId) -> RpcResult<()>;
    /// The user wants more on a brief: a `[meeting deeper]` steer; the answer arrives as `Brief{source: Deeper}`.
    #[method(name = "meeting_deeper")]
    async fn meeting_deeper(&self, brief: meeting::BriefId) -> RpcResult<()>;
    /// A typed question: `[meeting request, typed]` steer; the answer arrives as `Brief{source: Typed}`.
    #[method(name = "meeting_request")]
    async fn meeting_request(&self, text: String) -> RpcResult<()>;
    /// Push-to-talk held/released (53 decision 10): `Listening`; the release tags the
    /// mic utterance as a spoken request and steers it at once.
    #[method(name = "meeting_ptt")]
    async fn meeting_ptt(&self, held: bool) -> RpcResult<()>;
    /// Where the island's cursor is; mirrored to `meeting_status.cursor`.
    #[method(name = "meeting_cursor")]
    async fn meeting_cursor(&self, brief: Option<meeting::BriefId>) -> RpcResult<()>;
    /// Every brief of a meeting, in order (memory while recording, `briefs.jsonl` after).
    #[method(name = "meeting_briefs")]
    async fn meeting_briefs(&self, id: meeting::MeetingId) -> RpcResult<Vec<meeting::Brief>>;
    /// The current meeting's live topic buttons.
    #[method(name = "meeting_topics")]
    async fn meeting_topics(&self) -> RpcResult<Vec<meeting::Topic>>;
    /// A meeting's live transcript (`live.jsonl`, last 2 000 utterances).
    #[method(name = "meeting_live")]
    async fn meeting_live(&self, id: meeting::MeetingId) -> RpcResult<Vec<meeting::Utterance>>;
    /// Phase 4 (04 §5). The transcript of record (`transcript.json`), once the post pipeline is done.
    #[method(name = "meeting_transcript")]
    async fn meeting_transcript(&self, id: meeting::MeetingId) -> RpcResult<meeting::Transcript>;
    /// Apply review edits in order — all or none — and return the new transcript.
    #[method(name = "meeting_edit")]
    async fn meeting_edit(&self, id: meeting::MeetingId, ops: Vec<meeting::EditOp>) -> RpcResult<meeting::Transcript>;
    /// `[t0, t1)` of one track as a base64 16-bit mono WAV (48 kHz), for the review pane's player.
    #[method(name = "meeting_segment_audio")]
    async fn meeting_segment_audio(&self, id: meeting::MeetingId, track: meeting::Track, t0_ms: u64, t1_ms: u64) -> RpcResult<String>;
    /// Enrolled speaker profiles (embedding omitted).
    #[method(name = "meeting_speakers")]
    async fn meeting_speakers(&self) -> RpcResult<Vec<meeting::Profile>>;
    /// Delete a speaker profile file. Transcripts keep their labels.
    #[method(name = "meeting_speaker_delete")]
    async fn meeting_speaker_delete(&self, profile: meeting::ProfileId) -> RpcResult<()>;
    /// `Review` → `Summarizing`: the guided summary turn; `Summarized{title}` and `State{Done}` follow.
    #[method(name = "meeting_summarize")]
    async fn meeting_summarize(&self, id: meeting::MeetingId) -> RpcResult<()>;
    /// Re-run the post pipeline from the Opus files.
    #[method(name = "meeting_post_retry")]
    async fn meeting_post_retry(&self, id: meeting::MeetingId) -> RpcResult<()>;
    /// `summary.md`.
    #[method(name = "meeting_summary")]
    async fn meeting_summary(&self, id: meeting::MeetingId) -> RpcResult<String>;
    /// 57 §3.4: the meeting families' decision-oracle switch — `mode` as configured,
    /// `effective` (`off` with `reason` when the plugin is disabled or the checkpoint is
    /// missing), and the tier budgets.
    #[method(name = "meeting_oracle_mode")]
    async fn meeting_oracle_mode(&self) -> RpcResult<decision::ModeInfo>;
    /// Persist `decision.families.meeting: { mode }` (`off` | `shadow` | `active`) and apply
    /// it live; effective from the next batch. A failed config write changes nothing.
    #[method(name = "set_meeting_oracle_mode")]
    async fn set_meeting_oracle_mode(&self, mode: decision::Mode) -> RpcResult<decision::ModeInfo>;
    /// 57 §7: today's decision log folded per family — mode, asks, accepts, escalations by
    /// reason, tier-2 rows, p50/p95, shadow agreement.
    #[method(name = "decision_stats")]
    async fn decision_stats(&self) -> RpcResult<Vec<decision::FamilyStats>>;
    /// 57 §11.4: whether the families under `prefix` (`meeting`) capture training examples
    /// locally, and how much is on disk (`<data_dir>/decisions/training/<prefix>/`).
    #[method(name = "decision_training")]
    async fn decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo>;
    /// Persist `decision.families.<prefix>: { mode, train }` and apply it live; effective from
    /// the next batch. A failed config write changes nothing.
    #[method(name = "set_decision_training")]
    async fn set_decision_training(&self, prefix: String, on: bool) -> RpcResult<decision::TrainingInfo>;
    /// Phase 5 (05 §8): the brief cache. Full-text search, best rank first (`limit` ≤ 50, default 8).
    #[method(name = "brief_search")]
    async fn brief_search(&self, query: String, limit: Option<u32>) -> RpcResult<Vec<meeting::CachedBrief>>;
    /// Cached briefs, newest `updated_at` first, optionally only those tagged `tag` (`limit` ≤ 50).
    #[method(name = "brief_list")]
    async fn brief_list(&self, tag: Option<String>, limit: Option<u32>) -> RpcResult<Vec<meeting::CachedBrief>>;
    /// Every tag in the cache, most-used first.
    #[method(name = "brief_tags")]
    async fn brief_tags(&self) -> RpcResult<Vec<String>>;
    /// One cached brief.
    #[method(name = "brief_get")]
    async fn brief_get(&self, id: meeting::BriefId) -> RpcResult<meeting::CachedBrief>;
    /// Create (`id` omitted) or update a cached brief; `BriefEvent::Changed` follows. Source `user`.
    #[method(name = "brief_put")]
    async fn brief_put(&self, put: meeting::CachedBriefPut) -> RpcResult<meeting::CachedBrief>;
    /// Delete a cached brief; `BriefEvent::Changed { removed: true }` follows.
    #[method(name = "brief_delete")]
    async fn brief_delete(&self, id: meeting::BriefId) -> RpcResult<()>;
    /// Fire a schedule now, ignoring `next`; returns its session (34 §8).
    /// Refused while a run is in flight (`busy`) or the file does not parse.
    #[method(name = "fire_schedule")]
    async fn fire_schedule(&self, name: String) -> RpcResult<SessionId>;
    /// Flip a schedule's `enabled:` line (33 §3.5): pause/resume a loop.
    /// `Err` when unknown.
    #[method(name = "enable_schedule")]
    async fn enable_schedule(
        &self,
        name: String,
        enabled: bool,
    ) -> RpcResult<schedule::ScheduleInfo>;
    /// The last `limit` fires and skips of `name`, newest first (34 §8).
    #[method(name = "schedule_history")]
    async fn schedule_history(&self, name: String, limit: u32)
    -> RpcResult<Vec<schedule::RunInfo>>;
    /// Every artifact shown in `session` (`null` = every session), newest
    /// first; `descendants: true` (default `false`) widens to the session's
    /// subtree (35 §3.5).
    #[method(name = "list_artifacts", with_extensions)]
    async fn list_artifacts(
        &self,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<artifact::Artifact>>;
    /// Copy artifact `id`'s blob to `dest` (parent created; refuses to
    /// overwrite); returns the written path (35 §3.5).
    #[method(name = "export_artifact")]
    async fn export_artifact(&self, id: String, dest: PathBuf) -> RpcResult<PathBuf>;
    /// A committed app record: its artifact, source, compatibility hash,
    /// pinned tool bindings and diagnostics (46 §6.3, §8.3). Rejects with
    /// `-32001` when the app plugin is disabled: the client falls back to
    /// the artifact's readable source.
    #[method(name = "get_app")]
    async fn get_app(&self, record_call_id: String) -> RpcResult<AppDoc>;
    /// Run one pinned Query/Mutation binding of an open app view on behalf of
    /// the calling connection (46 §6). A `press` trigger is the approval; the
    /// tool name comes from the binding, never the client. `-32002` when no
    /// authenticated connection carries the call.
    #[method(name = "app_call_tool", with_extensions)]
    async fn app_call_tool(&self, args: CallArgs) -> RpcResult<AppToolResult>;
    /// Status (and stored result) of a pressed invocation by `request_id`,
    /// for reconnect and duplicate-press resolution (46 §6.4).
    #[method(name = "app_call_status", with_extensions)]
    async fn app_call_status(&self, request_id: String) -> RpcResult<AppToolResult>;
    /// Close one open view: stop its reads, drop queued work (46 §7). Idempotent.
    #[method(name = "app_close", with_extensions)]
    async fn app_close(&self, instance_id: String) -> RpcResult<()>;
    /// The artifact record of one `show` call, for opening a search hit
    /// without listing everything; vanished record is an error (49 §4.2).
    #[method(name = "get_artifact")]
    async fn get_artifact(&self, call_id: String) -> RpcResult<artifact::Artifact>;
    /// Up to 2,000 characters of the artifact's indexed text around the
    /// occurrence `query` matches, with highlight ranges and content coverage;
    /// unavailable extraction is an error, never the live `origin` (49 §7).
    #[method(name = "artifact_search_match")]
    async fn artifact_search_match(
        &self,
        call_id: String,
        query: String,
    ) -> RpcResult<ArtifactMatchContext>;
    /// Every secret's name, note and version history (37 §3.1). Never values.
    #[method(name = "list_secrets")]
    async fn list_secrets(&self) -> RpcResult<secrets::SecretsList>;
    /// Store a new version of `name` as `source: user`; `note: None` keeps
    /// the old note. Unchanged value → the current info, nothing written.
    #[method(name = "set_secret")]
    async fn set_secret(
        &self,
        name: String,
        value: String,
        note: Option<String>,
    ) -> RpcResult<secrets::SecretInfo>;
    /// Copy version `back` (1 = previous … 4) of `name` to the top as a new version.
    #[method(name = "restore_secret")]
    async fn restore_secret(&self, name: String, back: u8) -> RpcResult<secrets::SecretInfo>;
    /// Remove `name` and every kept version. `Err` when unknown.
    #[method(name = "delete_secret")]
    async fn delete_secret(&self, name: String) -> RpcResult<()>;
    /// Answer a `SecretsEvent::Requested`: `value: Some` stores it as
    /// `source: user`, `None` declines. `Err` when `id` is unknown or expired.
    #[method(name = "answer_secret")]
    async fn answer_secret(&self, id: secrets::RequestId, value: Option<String>) -> RpcResult<()>;
    /// Stream every wire-visible event as `{"type": ..., ...}`, optionally
    /// only those carrying `session`. `descendants: true` (default `false`)
    /// also delivers events of every session in `session`'s subtree, plus
    /// `SubagentSpawned` whose `parent` is in it (19 §2.2). A slow client sees
    /// `{"type":"Lagged","count":n}` where `n` events were dropped.
    #[subscription(name = "subscribe_events", unsubscribe = "unsubscribe_events", item = serde_json::Value, with_extensions)]
    async fn subscribe_events(
        &self,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult;
}

/// Members of private trees this process knows: `member → (root, owner
/// peer)`. The event filter drops any event naming a member for every peer
/// but its owner (51 §4.2). Insert-only: broadcast fan-out hands every
/// subscription task the same event in no fixed order, so removing an entry
/// on `SessionDeleted` / leave would let the *next* task pass that event to a
/// non-owner; ids are UUIDs and never reused, so a dead entry is harmless.
/// One per process so the TUI's in-process `Server` and `serve`'s share it.
pub type PrivateMap = Arc<RwLock<HashMap<SessionId, (SessionId, u64)>>>;

static PRIVATE: LazyLock<PrivateMap> = LazyLock::new(Default::default);

/// Server state: the event feed and the runtime (for `shutdown`).
pub struct Server {
    /// Feed from [`crate::bridge`].
    pub events: broadcast::Sender<Event>,
    /// `rt.shutdown` is cancelled by `shutdown`.
    pub rt: Runtime,
    /// Private-tree membership for admission of events; see [`PrivateMap`].
    pub private: PrivateMap,
}

impl Server {
    /// A server with no event feed: for in-process callers (the TUI), which
    /// subscribe to the bus themselves. `serve` builds the real one.
    pub fn new(rt: Runtime) -> Self {
        Self {
            events: broadcast::channel(1).0,
            rt,
            private: PRIVATE.clone(),
        }
    }

    /// The one admission check (51 §4.2): the calling connection may act on
    /// `session`. Durable sessions admit any authenticated peer; a private
    /// one only its owner; unknown or closing ids fail closed.
    async fn authorize(&self, ext: &Extensions, session: SessionId) -> RpcResult<Authorized> {
        let peer = peer(ext)?.id();
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Authorize {
                session,
                peer: Some(peer),
            },
        )
        .await
    }

    /// `authorize` when a session is named; `None` is the caller's own root
    /// or the global scope, nothing to admit.
    async fn authorize_opt(&self, ext: &Extensions, session: Option<SessionId>) -> RpcResult<()> {
        if let Some(s) = session {
            self.authorize(ext, s).await?;
        }
        Ok(())
    }

    /// Record a freshly opened private tree as `peer`'s: every known member
    /// of its root moves to `peer` (a resume rebinds the whole tree), and
    /// the opened session and root join.
    fn track(&self, open: &SessionOpen, peer: u64) {
        if open.retention != Retention::Private {
            return;
        }
        let mut map = self.private.write();
        for v in map.values_mut() {
            if v.0 == open.root {
                v.1 = peer;
            }
        }
        map.insert(open.root, (open.root, peer));
        map.insert(open.session, (open.root, peer));
    }
}

pub(crate) fn lookup<A: Actor>(name: &str) -> RpcResult<ActorRef<A>> {
    match ActorRef::<A>::lookup(name) {
        Ok(Some(a)) => Ok(a),
        Ok(None) => Err(ErrorObjectOwned::owned(
            NOT_RUNNING,
            format!("{name} not running"),
            None::<()>,
        )),
        Err(e) => Err(ErrorObjectOwned::owned(
            ASK_FAILED,
            e.to_string(),
            None::<()>,
        )),
    }
}

/// Look up `name`, `ask` it `msg`, map every failure to a JSON-RPC error.
pub(crate) async fn ask<A, M>(name: &str, msg: M) -> RpcResult<<A::Reply as Reply>::Ok>
where
    A: Actor + Message<M>,
    M: Send + 'static,
    <A::Reply as Reply>::Error: Display,
{
    send(&lookup::<A>(name)?, msg).await
}

/// `ask` a resolved actor; handler errors and a stopped actor become `ASK_FAILED`.
async fn send<A, M>(actor: &ActorRef<A>, msg: M) -> RpcResult<<A::Reply as Reply>::Ok>
where
    A: Actor + Message<M>,
    M: Send + 'static,
    <A::Reply as Reply>::Error: Display,
{
    actor.ask(msg).await.map_err(|e: SendError<M, _>| {
        ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>)
    })
}

/// The nodes actor; absent when `nodes.enabled: false` or the plugin is not loaded.
fn nodes() -> RpcResult<ActorRef<nodes::NodesActor>> {
    lookup::<nodes::NodesActor>(nodes::NODES_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "nodes is disabled", None::<()>),
        _ => e,
    })
}

/// The schedule actor; absent when `schedule.enabled: false` or the plugin is not loaded.
fn schedule() -> RpcResult<ActorRef<schedule::Schedule>> {
    lookup::<schedule::Schedule>(schedule::SCHEDULE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "schedule is disabled", None::<()>),
        _ => e,
    })
}

/// The update actor; absent when `update.enabled: false` or the plugin is not loaded.
fn update_actor() -> RpcResult<ActorRef<update::UpdateActor>> {
    lookup::<update::UpdateActor>(update::UPDATE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "update is disabled", None::<()>),
        _ => e,
    })
}

/// The voice actor; absent when `voice.enabled: false` or the plugin is not loaded.
fn voice() -> RpcResult<ActorRef<voice::Voice>> {
    lookup::<voice::Voice>(voice::VOICE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "voice is disabled", None::<()>),
        _ => e,
    })
}

/// The STT actor; absent when `voice.enabled: false` or the plugin is not loaded.
fn transcriber() -> RpcResult<ActorRef<voice::Transcriber>> {
    lookup::<voice::Transcriber>(voice::TRANSCRIBER_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "voice is disabled", None::<()>),
        _ => e,
    })
}

/// The dictation actor; absent when `voice.enabled: false` or the plugin is not loaded.
fn dictation() -> RpcResult<ActorRef<voice::Dictation>> {
    lookup::<voice::Dictation>(voice::DICTATION_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "voice is disabled", None::<()>),
        _ => e,
    })
}

/// The meeting actor; absent when `meeting.enabled: false`, off macOS, or the plugin is not loaded.
fn meeting() -> RpcResult<ActorRef<meeting::Meeting>> {
    lookup::<meeting::Meeting>(meeting::MEETING_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "meetings are disabled", None::<()>),
        _ => e,
    })
}

/// The artifact actor; absent when `artifact.enabled: false` or the plugin is not loaded.
fn artifact() -> RpcResult<ActorRef<artifact::ArtifactActor>> {
    lookup::<artifact::ArtifactActor>(artifact::ARTIFACT_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "artifact is disabled", None::<()>),
        _ => e,
    })
}

/// The app actor; absent when `app.enabled: false` or the plugin is not loaded.
fn apps() -> RpcResult<ActorRef<Apps>> {
    lookup::<Apps>(app::APP_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "app is disabled", None::<()>),
        _ => e,
    })
}

/// The authenticated connection behind a `with_extensions` call. Both
/// listeners (local socket, link conn) put one in every admitted request;
/// its absence means nothing authenticated the call — never "local".
fn peer(ext: &Extensions) -> RpcResult<link::RpcPeer> {
    ext.get::<link::RpcPeer>().cloned().ok_or_else(|| {
        ErrorObjectOwned::owned(UNAUTHORIZED, "no authenticated connection", None::<()>)
    })
}

/// The secrets actor; absent when the plugin is not loaded or the vault failed to open.
fn secrets() -> RpcResult<ActorRef<secrets::SecretsActor>> {
    lookup::<secrets::SecretsActor>(secrets::SECRETS_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => {
            ErrorObjectOwned::owned(NOT_RUNNING, "secrets vault unavailable", None::<()>)
        }
        _ => e,
    })
}

/// The knowledge actor; `knowledge.enabled: false` means it was never
/// spawned (12 §9.3).
fn knowledge() -> RpcResult<ActorRef<KnowledgeActor>> {
    lookup::<KnowledgeActor>(KNOWLEDGE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "knowledge is disabled", None::<()>),
        _ => e,
    })
}

/// The A2A actor, or `None` for the one silent case: `a2a.enabled: false`
/// (or no `a2a` section) and nothing registered. A malformed section, or
/// `enabled: true` without a registered actor (plugin missing, crashed), is
/// an error the owner must see, never a fake "disabled".
fn a2a(rt: &Runtime) -> RpcResult<Option<ActorRef<A2aActor>>> {
    match lookup::<A2aActor>(A2A_ACTOR) {
        Ok(actor) => return Ok(Some(actor)),
        Err(e) if e.code() != NOT_RUNNING => return Err(e),
        Err(_) => {}
    }
    let enabled = if rt.config.find_value("a2a").is_ok() {
        rt.section::<a2a::config::A2aConfig>("a2a")
            .map_err(|e| failed(format!("a2a config: {e:#}")))?
            .enabled
    } else {
        false
    };
    if enabled {
        return Err(ErrorObjectOwned::owned(
            NOT_RUNNING,
            "a2a is enabled but its actor is not running",
            None::<()>,
        ));
    }
    Ok(None)
}

/// The owner console's root: `NewRootSession { scope: Owner }`, trusted
/// because the caller authenticated as the owner over the socket or is the
/// in-process TUI. Never promotes an existing root (50 §2.3).
async fn owner_root() -> RpcResult<SessionId> {
    ask::<Agents, _>(
        agent::AGENT_ACTOR,
        NewRootSession {
            cwd: None,
            scope: PeerScope::Owner,
        },
    )
    .await
}

/// The link actor; absent when the plugin is not in this process's list.
fn link() -> RpcResult<ActorRef<LinkActor>> {
    lookup::<LinkActor>(LINK_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "link is disabled", None::<()>),
        _ => e,
    })
}

/// This machine's hostname, read once; `"theuth"` when `hostname` fails.
static HOSTNAME: LazyLock<String> = LazyLock::new(|| {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "theuth".to_owned())
});

/// Write `bytes` as `<data_dir>/attachments/<sha256[..16]>.<ext>`; an existing
/// file is not rewritten. Shared by `put_attachment` and the ACP prompt path;
/// the naming rule lives in `agent::context` (48 §3.3).
pub(crate) fn save_blob(bytes: &[u8], ext: &str, data_dir: &Path) -> std::io::Result<PathBuf> {
    agent::context::save_blob(bytes, ext, data_dir)
}

/// An `ASK_FAILED` (`-32000`) error with `msg`.
fn failed(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(ASK_FAILED, msg.into(), None::<()>)
}

/// Largest decoded image `put_attachment` accepts (17 §1: Anthropic's 5 MB).
const MAX_IMAGE: usize = 5 * 1024 * 1024;
/// Largest decoded PDF `put_attachment` accepts (20 §2.5: Anthropic's 32 MB).
const MAX_PDF: usize = 32 * 1024 * 1024;
/// Largest decoded audio note `put_attachment` accepts (45 §7.1).
const MAX_AUDIO: usize = 16 * 1024 * 1024;

/// Validate `ext` and the size cap for its kind, then decode base64 `bytes`
/// (`put_attachment`, `put_session_attachment`).
fn decode_attachment(bytes: String, ext: &str) -> RpcResult<Vec<u8>> {
    let cap = match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => MAX_IMAGE,
        "pdf" => MAX_PDF,
        "m4a" | "mp4" | "webm" | "ogg" | "opus" | "mp3" | "wav" => MAX_AUDIO,
        _ => return Err(failed(format!("unsupported attachment type .{ext}"))),
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes)
        .map_err(|e| failed(format!("bad base64: {e}")))?;
    if bytes.len() > cap {
        return Err(failed(format!(
            "attachment is {} bytes; the cap for .{ext} is {cap}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

/// `PtyAttach` as an RPC result; `size` resizes a running job first.
async fn attach_pty(session: SessionId, job: &JobId, size: Option<(u16, u16)>) -> RpcResult<tools::PtyAttached> {
    lookup::<Jobs>(JOBS_ACTOR)?
        .ask(PtyAttach { session, id: job.clone(), size })
        .await
        .map_err(|e| match e {
            SendError::HandlerError(msg) => failed(msg),
            other => failed(other.to_string()),
        })
}

#[async_trait]
impl ApiServer for Server {
    async fn info(&self) -> RpcResult<Info> {
        let default_model = self
            .rt
            .section::<AgentConfig>("agent")
            .unwrap_or_default()
            .model;
        let approvals =
            ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetApprovals { session: None }).await?;
        let compact_style = ask::<Agents, _>(agent::AGENT_ACTOR, GetCompactStyle).await?;
        let subagent_timeout_secs =
            ask::<Agents, _>(agent::AGENT_ACTOR, GetSubagentTimeout).await?;
        let max_tool_rounds = ask::<Agents, _>(agent::AGENT_ACTOR, GetMaxToolRounds).await?;
        let stream_idle_secs = ask::<Agents, _>(agent::AGENT_ACTOR, GetStreamIdle).await?;
        let max_salvages = ask::<Agents, _>(agent::AGENT_ACTOR, GetMaxSalvages).await?;
        let pty_idle_secs = ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetPtyIdle).await?;
        let name = match self.rt.section::<LinkConfig>("link") {
            Ok(c) if !c.name.is_empty() => c.name,
            _ => HOSTNAME.clone(),
        };
        let communication =
            ask::<Agents, _>(agent::AGENT_ACTOR, GetCommunication { session: None }).await?;
        Ok(Info {
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            data_dir: self.rt.data_dir.display().to_string(),
            default_model,
            approvals,
            compact_style,
            subagent_timeout_secs,
            max_tool_rounds,
            stream_idle_secs,
            max_salvages,
            pty_idle_secs,
            name,
            communication,
            app_enabled: apps().is_ok(),
        })
    }

    async fn new_session(&self) -> RpcResult<SessionId> {
        owner_root().await
    }

    async fn send_input(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<SessionId> {
        self.authorize_opt(ext, session).await?;
        let session = match session {
            Some(s) => s,
            None => owner_root().await?,
        };
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SendInput {
                session: Some(session),
                text,
                files,
            },
        )
        .await
    }

    async fn steer(
        &self,
        ext: &Extensions,
        session: SessionId,
        text: String,
        id: Option<InputId>,
        files: Vec<PathBuf>,
    ) -> RpcResult<InputId> {
        self.authorize(ext, session).await?;
        let id = id.unwrap_or_default();
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Steer {
                session,
                id,
                text,
                files,
            },
        )
        .await?;
        Ok(id)
    }

    async fn withdraw(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: InputId,
    ) -> RpcResult<String> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, Withdraw { session, id }).await
    }

    async fn list_queued(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<QueuedInput>> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, ListQueued { session }).await
    }

    async fn cancel(&self, ext: &Extensions, session: SessionId) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, Cancel { session }).await
    }

    async fn list_jobs(&self, ext: &Extensions, session: SessionId) -> RpcResult<Vec<JobView>> {
        self.authorize(ext, session).await?;
        ask::<Jobs, _>(JOBS_ACTOR, ListJobs(session)).await
    }

    async fn cancel_job(&self, ext: &Extensions, session: SessionId, id: JobId) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Jobs, _>(JOBS_ACTOR, CancelJob { session, id }).await
    }

    async fn list_terminals(
        &self,
        ext: &Extensions,
        session: SessionId,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<TerminalSummary>> {
        self.authorize(ext, session).await?;
        let mut sessions = vec![session];
        if descendants == Some(true) {
            let agents = ask::<Agents, _>(agent::AGENT_ACTOR, ListAgents { session }).await?;
            sessions.extend(agents.into_iter().map(|a| a.id));
        }
        ask::<Jobs, _>(JOBS_ACTOR, ListTerminals { sessions }).await
    }

    async fn pty_screen(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        viewport: PtyViewport,
        if_revision: Option<u64>,
    ) -> RpcResult<PtyScreenReply> {
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyScreen {
                session,
                id: job,
                viewport,
                if_revision,
            })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(busy) => failed(format!("terminal busy: {busy}")),
                other => failed(other.to_string()),
            })
    }

    async fn pty_input(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        text: Option<String>,
        keys: Option<Vec<Key>>,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyInput {
                session,
                id: job,
                text,
                keys: keys.unwrap_or_default(),
            })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(msg) => failed(msg),
                other => failed(other.to_string()),
            })
    }

    async fn pty_resize(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        rows: u16,
        cols: u16,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyResize { session, id: job, rows, cols })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(msg) => failed(msg),
                other => failed(other.to_string()),
            })
    }

    async fn subscribe_pty(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        rows: Option<u16>,
        cols: Option<u16>,
    ) -> SubscriptionResult {
        let admit = async {
            self.authorize(ext, session).await?;
            attach_pty(session, &job, rows.zip(cols)).await
        };
        let mut at = match admit.await {
            Ok(a) => a,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let item = |c: &PtyChunk| SubscriptionMessage::from(serde_json::value::to_raw_value(c).expect("PtyChunk serializes"));
        tokio::spawn(async move {
            loop {
                let reset = PtyChunk::Reset { data: b64(&at.replay), rows: at.rows, cols: at.cols };
                if sink.send(item(&reset)).await.is_err() {
                    return;
                }
                if let Some(exit) = at.exit {
                    let _ = sink.send(item(&PtyChunk::Exit { exit })).await;
                    return;
                }
                loop {
                    let out = tokio::select! {
                        _ = sink.closed() => return,
                        r = at.rx.recv() => r,
                    };
                    let chunk = match out {
                        Ok(PtyOut::Data(bytes)) => PtyChunk::Data { data: b64(&bytes) },
                        Ok(PtyOut::Exit(exit)) => {
                            let _ = sink.send(item(&PtyChunk::Exit { exit })).await;
                            return;
                        }
                        // Behind by more than the ring: start over from a snapshot.
                        Err(broadcast::error::RecvError::Lagged(_)) => break,
                        Err(broadcast::error::RecvError::Closed) => return,
                    };
                    if sink.send(item(&chunk)).await.is_err() {
                        return;
                    }
                }
                at = match attach_pty(session, &job, None).await {
                    Ok(a) => a,
                    Err(_) => return,
                };
            }
        });
        Ok(())
    }

    async fn browser_screenshot(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Option<String>> {
        self.authorize(ext, session).await?;
        ask::<Browsers, _>(BROWSER_ACTOR, Screenshot { session }).await
    }

    async fn computer_permissions(&self) -> RpcResult<computer::PermissionStatus> {
        ask::<Computer, _>(COMPUTER_ACTOR, Permissions).await
    }

    async fn compact(
        &self,
        ext: &Extensions,
        session: SessionId,
        style: Option<CompactStyle>,
    ) -> RpcResult<u32> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Compact {
                session,
                style,
                head: None,
            },
        )
        .await
    }

    async fn undo(&self, ext: &Extensions, session: SessionId) -> RpcResult<String> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, Undo { session }).await
    }

    async fn redo(&self, ext: &Extensions, session: SessionId) -> RpcResult<String> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, Redo { session }).await
    }

    async fn context_items(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<ContextSnapshot> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, ContextItems { session }).await
    }

    async fn remove_context(
        &self,
        ext: &Extensions,
        session: SessionId,
        expected_revision: String,
        targets: Vec<ContextTarget>,
    ) -> RpcResult<ContextRemoval> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            RemoveContext {
                session,
                expected_revision,
                targets,
            },
        )
        .await
    }

    async fn regenerate(&self, ext: &Extensions, session: SessionId, turn: i64) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, Regenerate { session, turn }).await
    }

    async fn edit_turn(
        &self,
        ext: &Extensions,
        session: SessionId,
        turn: i64,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            EditTurn {
                session,
                turn,
                text,
                files,
            },
        )
        .await
    }

    async fn fork_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        through: Option<i64>,
        reply: Option<u32>,
    ) -> RpcResult<SessionId> {
        let source = self.authorize(ext, session).await?;
        let forked = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ForkSession {
                session,
                through,
                reply,
            },
        )
        .await?;
        // A private fork is a new private root owned by the same connection
        // (51 §4.1): admit its events to this peer only.
        if source.retention == Retention::Private {
            let owner = peer(ext)?.id();
            self.private.write().insert(forked, (forked, owner));
        }
        Ok(forked)
    }

    async fn compact_style(&self) -> RpcResult<CompactStyle> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetCompactStyle).await
    }

    async fn set_compact_style(&self, style: CompactStyle) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetCompactStyle { style }).await
    }

    async fn set_subagent_timeout(&self, seconds: u64) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetSubagentTimeout { seconds }).await
    }

    async fn set_max_tool_rounds(&self, rounds: u32) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetMaxToolRounds { rounds }).await
    }

    async fn set_stream_idle(&self, seconds: u64) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetStreamIdle { seconds }).await
    }

    async fn set_max_salvages(&self, count: u32) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetMaxSalvages { count }).await
    }

    async fn set_pty_idle(&self, seconds: u64) -> RpcResult<()> {
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, SetPtyIdle { seconds }).await
    }

    async fn title_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetTitleModel).await
    }

    async fn set_title_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetTitleModel { model }).await
    }

    async fn vision_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetVisionModel).await
    }

    async fn set_vision_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetVisionModel { model }).await
    }

    async fn escalate_model(&self) -> RpcResult<EscalateModel> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetEscalateModel).await
    }

    async fn set_escalate_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetEscalateModel { model }).await
    }

    async fn subagent_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetSubagentModel).await
    }

    async fn set_subagent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetSubagentModel { model }).await
    }

    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions).await
    }

    async fn list_agents(&self, ext: &Extensions, session: SessionId) -> RpcResult<Vec<AgentInfo>> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, ListAgents { session }).await
    }

    async fn load_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        before: Option<i64>,
        limit: Option<u32>,
        settled_only: Option<bool>,
    ) -> RpcResult<Loaded> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            LoadSession {
                session,
                before,
                limit,
                settled_only: settled_only.unwrap_or(false),
            },
        )
        .await
    }

    async fn get_todos(&self, ext: &Extensions, session: SessionId) -> RpcResult<TodoSnapshot> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, GetTodos { session }).await
    }

    async fn session_cost(&self, ext: &Extensions, session: SessionId) -> RpcResult<SessionCost> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SessionCostQuery(session))
            .await?
            .ok_or_else(|| failed(format!("{session}: no accounting owner")))
    }

    async fn rename_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        title: String,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, RenameSession { session, title }).await
    }

    async fn pin_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        pinned: bool,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, PinSession { session, pinned }).await
    }

    async fn delete_session(&self, ext: &Extensions, session: SessionId) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, DeleteSession { session }).await
    }

    async fn create_session(
        &self,
        ext: &Extensions,
        options: CreateSessionOptions,
    ) -> RpcResult<SessionOpen> {
        let peer = peer(ext)?;
        let open = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            CreateSession {
                options,
                peer: Some((peer.id(), peer.closed_token())),
            },
        )
        .await?;
        self.track(&open, peer.id());
        Ok(open)
    }

    async fn resume_private(
        &self,
        ext: &Extensions,
        session: SessionId,
        view: String,
        lease_secret: String,
    ) -> RpcResult<SessionOpen> {
        let peer = peer(ext)?;
        let open = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ResumePrivate {
                session,
                view,
                lease_secret,
                peer: (peer.id(), peer.closed_token()),
            },
        )
        .await?;
        self.track(&open, peer.id());
        Ok(open)
    }

    async fn leave_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        lease_secret: Option<String>,
    ) -> RpcResult<CloseResult> {
        let peer = peer(ext)?.id();
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            LeaveSession {
                session,
                lease_secret,
                peer,
            },
        )
        .await
    }

    async fn set_session_project(
        &self,
        ext: &Extensions,
        session: SessionId,
        project: Option<String>,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SetSessionProject { session, project }).await
    }

    async fn put_session_attachment(
        &self,
        ext: &Extensions,
        session: SessionId,
        bytes: String,
        file_ext: String,
    ) -> RpcResult<PathBuf> {
        // Ownership first: nothing is decoded or written for a session the
        // caller may not touch (51 §8.1).
        let authorized = self.authorize(ext, session).await?;
        let bytes = decode_attachment(bytes, &file_ext)?;
        save_blob(&bytes, &file_ext, &authorized.resources.root).map_err(|e| failed(e.to_string()))
    }

    async fn search(
        &self,
        query: String,
        categories: Option<Vec<SearchCategory>>,
        limit: Option<u32>,
    ) -> RpcResult<SearchResponse> {
        crate::search::run(&self.rt, query, categories, limit)
            .await
            .map_err(|e| {
                ErrorObjectOwned::owned(jsonrpsee::types::error::INVALID_PARAMS_CODE, e, None::<()>)
            })
    }

    async fn search_refresh(&self, categories: Option<Vec<SearchCategory>>) -> RpcResult<()> {
        crate::search::refresh(categories)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e, None::<()>))
    }

    async fn list_tools(&self) -> RpcResult<Vec<ToolSpec>> {
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, ListTools(None)).await
    }

    async fn resolve_approval(&self, id: ApprovalId, decision: Decision) -> RpcResult<bool> {
        ask::<Approval, _>(tools::APPROVAL_ACTOR, Resolve { id, decision }).await
    }

    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> RpcResult<bool> {
        agent::answer_ask(id, answers)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>))
    }

    async fn list_models(&self) -> RpcResult<Vec<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListModels).await
    }

    async fn list_aws_profiles(&self) -> RpcResult<AwsProfiles> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListAwsProfiles).await
    }

    async fn set_aws_profile(&self, profile: Option<String>) -> RpcResult<AwsProfiles> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetAwsProfile { profile }).await
    }

    async fn set_model(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        model: String,
    ) -> RpcResult<ModelInfo> {
        self.authorize_opt(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SetModel { session, model }).await
    }

    async fn set_effort(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        effort: Option<String>,
    ) -> RpcResult<()> {
        self.authorize_opt(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SetEffort { session, effort }).await
    }

    async fn list_mcp(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<Vec<McpServer>> {
        self.authorize_opt(ext, session).await?;
        ask::<Mcp, _>(tools::MCP_ACTOR, ListServers { session }).await
    }

    async fn mcp_status(&self) -> RpcResult<McpProgress> {
        ask::<Mcp, _>(tools::MCP_ACTOR, Progress).await
    }

    async fn set_mcp_gate(
        &self,
        server: String,
        tool: Option<String>,
        gate: Option<Gate>,
    ) -> RpcResult<()> {
        ask::<Mcp, _>(tools::MCP_ACTOR, SetGate { server, tool, gate }).await
    }

    async fn set_mcp_hidden(
        &self,
        ext: &Extensions,
        session: SessionId,
        tools: Vec<String>,
        hidden: bool,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            SetHidden {
                session,
                tools,
                hidden,
            },
        )
        .await
    }

    async fn set_approvals(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<ApprovalsState> {
        self.authorize_opt(ext, session).await?;
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, SetApprovals { session, enabled }).await
    }

    async fn get_communication(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<CommunicationState> {
        self.authorize_opt(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, GetCommunication { session }).await
    }

    async fn set_communication(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<CommunicationState> {
        self.authorize_opt(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SetCommunication { session, enabled }).await
    }

    async fn set_peer_scope(
        &self,
        ext: &Extensions,
        root: SessionId,
        scope: PeerScope,
        alias: Option<String>,
    ) -> RpcResult<CommunicationState> {
        self.authorize(ext, root).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SetPeerScope { root, scope, alias }).await
    }

    async fn list_peers(
        &self,
        ext: &Extensions,
        session: SessionId,
        query: Option<String>,
        cursor: Option<String>,
        limit: Option<u16>,
    ) -> RpcResult<PeerPage> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ListPeers {
                session,
                query,
                cursor,
                limit,
            },
        )
        .await
    }

    async fn peer_message(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: Uuid,
    ) -> RpcResult<PeerRecord> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, GetPeerMessage { session, id }).await
    }

    async fn list_peer_messages(
        &self,
        ext: &Extensions,
        session: SessionId,
        cursor: Option<String>,
        limit: Option<u16>,
        pending_only: bool,
    ) -> RpcResult<PeerMessagePage> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ListPeerMessages {
                session,
                cursor,
                limit,
                pending_only,
            },
        )
        .await
    }

    async fn answer_peer_request(
        &self,
        ext: &Extensions,
        session: SessionId,
        reply_to: Uuid,
        text: String,
    ) -> RpcResult<PeerReceipt> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            AnswerPeerRequest {
                session,
                reply_to,
                text,
            },
        )
        .await
    }

    async fn get_prompt(&self, ext: &Extensions, session: Option<SessionId>) -> RpcResult<String> {
        self.authorize_opt(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, ShowPrompt { session }).await
    }

    async fn list_skills(&self) -> RpcResult<Vec<SkillMeta>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListSkills).await
    }

    async fn load_skill(&self, name: String) -> RpcResult<SkillBody> {
        ask::<Agents, _>(agent::AGENT_ACTOR, LoadSkill { name }).await
    }

    async fn use_skill(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        name: String,
        args: String,
    ) -> RpcResult<SessionId> {
        self.authorize_opt(ext, session).await?;
        let session = match session {
            Some(s) => s,
            None => owner_root().await?,
        };
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            UseSkill {
                session: Some(session),
                name,
                args,
            },
        )
        .await
    }

    async fn add_skill(
        &self,
        name: String,
        description: String,
        body: String,
        always: bool,
    ) -> RpcResult<SkillMeta> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            AddSkill {
                name,
                description,
                body,
                always,
            },
        )
        .await
    }

    async fn remove_skill(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveSkill { name }).await
    }

    async fn add_skill_repo(&self, url: String, name: Option<String>) -> RpcResult<Vec<SkillMeta>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, AddSkillRepo { url, name }).await
    }

    async fn remove_skill_repo(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveSkillRepo { name }).await
    }

    async fn sync_skills(&self) -> RpcResult<Vec<SyncResult>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SyncSkills).await
    }

    async fn knowledge_search(&self, query: String, limit: Option<u32>) -> RpcResult<Vec<Hit>> {
        send(&knowledge()?, Search { query, limit }).await
    }

    async fn knowledge_get(&self, id: String) -> RpcResult<Concept> {
        send(&knowledge()?, Get { id }).await
    }

    async fn knowledge_list(
        &self,
        prefix: Option<String>,
        type_: Option<String>,
        tag: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<Meta>> {
        send(
            &knowledge()?,
            List {
                prefix,
                type_,
                tag,
                limit,
            },
        )
        .await
    }

    async fn knowledge_put(&self, put: Put) -> RpcResult<Meta> {
        send(
            &knowledge()?,
            PutConcept {
                put,
                source: Source::User,
            },
        )
        .await
    }

    async fn knowledge_delete(&self, id: String) -> RpcResult<()> {
        send(&knowledge()?, Delete { id }).await
    }

    async fn knowledge_write(&self, id: String, text: String) -> RpcResult<Meta> {
        send(&knowledge()?, Write { id, text }).await
    }

    async fn knowledge_sync(&self) -> RpcResult<knowledge::Sync> {
        send(&knowledge()?, SyncNow).await
    }

    async fn knowledge_history(
        &self,
        id: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<History> {
        send(&knowledge()?, HistoryOf { id, limit }).await
    }

    async fn knowledge_revision(&self, commit: String, id: Option<String>) -> RpcResult<Revision> {
        send(&knowledge()?, RevisionOf { commit, id }).await
    }

    async fn knowledge_read_revision(
        &self,
        id: String,
        commit: String,
    ) -> RpcResult<HistoricalConcept> {
        send(
            &knowledge()?,
            knowledge::search::ReadRevision { id, commit },
        )
        .await
    }

    async fn knowledge_restore(&self, id: String, commit: Option<String>) -> RpcResult<Change> {
        send(&knowledge()?, Restore { id, commit }).await
    }

    async fn knowledge_harvest(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<HarvestResult> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, Harvest { session }).await
    }

    async fn add_mcp_server(&self, name: String, entry: serde_json::Value) -> RpcResult<McpServer> {
        ask::<Mcp, _>(tools::MCP_ACTOR, AddServer { name, entry }).await
    }

    async fn list_acp(&self, probe: bool) -> RpcResult<Vec<acp::EntryInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListAcp { probe }).await
    }

    async fn add_acp(&self, id: String) -> RpcResult<acp::EntryInfo> {
        ask::<Agents, _>(agent::AGENT_ACTOR, AddAcp { id }).await
    }

    async fn remove_acp(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveAcp { name }).await
    }

    async fn list_acp_registry(&self) -> RpcResult<Vec<acp::registry::RegistryAgent>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListRegistry).await
    }

    async fn remove_mcp_server(&self, name: String) -> RpcResult<()> {
        ask::<Mcp, _>(tools::MCP_ACTOR, RemoveServer { name }).await
    }

    async fn add_mcp_repo(&self, url: String, name: Option<String>) -> RpcResult<RepoInfo> {
        ask::<Mcp, _>(tools::MCP_ACTOR, AddRepo { url, name }).await
    }

    async fn sync_mcp(&self) -> RpcResult<Vec<SyncResult>> {
        ask::<Mcp, _>(tools::MCP_ACTOR, SyncMcp).await
    }

    async fn list_logins(&self) -> RpcResult<Vec<LoginInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListLogins).await
    }

    async fn login(&self, provider: String) -> RpcResult<String> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Login { provider }).await
    }

    async fn logout(&self, provider: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Logout { provider }).await
    }

    async fn list_providers(&self) -> RpcResult<Vec<ProviderInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListProviders).await
    }

    async fn add_provider(
        &self,
        name: String,
        base_url: String,
        key: String,
    ) -> RpcResult<ProviderInfo> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            AddProvider {
                name,
                base_url,
                key,
            },
        )
        .await
    }

    async fn remove_provider(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveProvider { name }).await
    }

    async fn mcp_login(&self, server: String) -> RpcResult<String> {
        ask::<Mcp, _>(tools::MCP_ACTOR, McpLogin { server }).await
    }

    async fn mcp_logout(&self, server: String) -> RpcResult<()> {
        ask::<Mcp, _>(tools::MCP_ACTOR, McpLogout { server }).await
    }

    async fn signal_link(&self) -> RpcResult<String> {
        ask::<Signal, _>(SIGNAL_ACTOR, Link).await
    }

    async fn signal_status(&self) -> RpcResult<SignalStatus> {
        ask::<Signal, _>(SIGNAL_ACTOR, Status).await
    }

    async fn signal_allow(&self, number: String) -> RpcResult<()> {
        ask::<Signal, _>(SIGNAL_ACTOR, Allow(number)).await
    }

    async fn signal_remove_allowed(&self, number: String) -> RpcResult<()> {
        ask::<Signal, _>(SIGNAL_ACTOR, RemoveAllowed(number)).await
    }

    async fn signal_unlink(&self) -> RpcResult<()> {
        ask::<Signal, _>(SIGNAL_ACTOR, Unlink).await
    }

    async fn slack_status(&self) -> RpcResult<SlackStatus> {
        ask::<Slack, _>(SLACK_ACTOR, slack::Status).await
    }

    async fn slack_allow(&self, user: String) -> RpcResult<()> {
        ask::<Slack, _>(SLACK_ACTOR, slack::Allow(user)).await
    }

    async fn slack_remove_allowed(&self, user: String) -> RpcResult<()> {
        ask::<Slack, _>(SLACK_ACTOR, slack::RemoveAllowed(user)).await
    }

    async fn slack_allow_channel(&self, channel: String) -> RpcResult<()> {
        ask::<Slack, _>(SLACK_ACTOR, slack::AllowChannel(channel)).await
    }

    async fn slack_remove_channel(&self, channel: String) -> RpcResult<()> {
        ask::<Slack, _>(SLACK_ACTOR, slack::RemoveChannel(channel)).await
    }

    async fn a2a_status(&self) -> RpcResult<A2aStatus> {
        match a2a(&self.rt)? {
            Some(actor) => send(&actor, a2a::GetStatus).await,
            None => Ok(A2aStatus::disabled()),
        }
    }

    async fn reload_a2a(&self) -> RpcResult<A2aStatus> {
        match a2a(&self.rt)? {
            Some(actor) => send(&actor, a2a::Reload).await,
            None => Ok(A2aStatus::disabled()),
        }
    }

    async fn shutdown(&self) -> RpcResult<()> {
        self.rt.shutdown.cancel();
        Ok(())
    }

    async fn link_status(&self) -> RpcResult<LinkStatus> {
        send(&link()?, link::Status).await
    }

    async fn link_enable(&self, enabled: bool, relay: Option<String>) -> RpcResult<()> {
        send(&link()?, Enable { enabled, relay }).await
    }

    async fn link_pair(&self) -> RpcResult<Pairing> {
        send(&link()?, Pair).await
    }

    async fn link_cancel_pair(&self) -> RpcResult<()> {
        send(&link()?, CancelPair).await
    }

    async fn link_rename(&self, name: String) -> RpcResult<()> {
        send(&link()?, Rename { name }).await
    }

    async fn link_rename_device(&self, id: String, name: String) -> RpcResult<()> {
        send(&link()?, RenameDevice { id, name }).await
    }

    async fn link_revoke_device(&self, id: String) -> RpcResult<()> {
        send(&link()?, Revoke { id }).await
    }

    async fn list_nodes(&self) -> RpcResult<Vec<nodes::Node>> {
        send(&nodes()?, nodes::Status).await
    }

    async fn add_node(&self, key: String, name: Option<String>) -> RpcResult<nodes::Node> {
        send(&nodes()?, nodes::Add { key, name }).await
    }

    async fn remove_node(&self, id: String) -> RpcResult<()> {
        send(&nodes()?, nodes::Remove { id }).await
    }

    async fn rename_node(&self, id: String, name: String) -> RpcResult<()> {
        send(&nodes()?, nodes::Rename { id, name }).await
    }

    async fn invite_node(&self, id: String) -> RpcResult<Pairing> {
        send(&nodes()?, nodes::Invite { id }).await
    }

    async fn list_schedules(&self) -> RpcResult<Vec<schedule::ScheduleInfo>> {
        send(&schedule()?, schedule::ListSchedules).await
    }

    async fn update_status(&self) -> RpcResult<Option<update::UpdateReady>> {
        send(&update_actor()?, update::Status).await
    }

    async fn speak(
        &self,
        ext: &Extensions,
        session: SessionId,
        text: String,
    ) -> RpcResult<SpeechId> {
        self.authorize(ext, session).await?;
        send(&voice()?, voice::Speak { session, text }).await
    }

    async fn stop_speaking(&self, id: Option<SpeechId>) -> RpcResult<()> {
        send(&voice()?, voice::Stop { id }).await
    }

    async fn voice_status(&self) -> RpcResult<VoiceStatus> {
        send(&voice()?, voice::Status).await
    }

    async fn transcribe(&self, path: PathBuf) -> RpcResult<voice::Transcript> {
        let real =
            std::fs::canonicalize(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let inbox = std::fs::canonicalize(self.rt.data_dir.join("attachments"));
        if !inbox.is_ok_and(|dir| real.starts_with(dir)) {
            return Err(failed(format!("{}: not an attachment", path.display())));
        }
        send(&transcriber()?, voice::Transcribe { path: real }).await
    }

    async fn dictation_prewarm(&self, ext: &Extensions) -> RpcResult<voice::ModelState> {
        peer(ext)?;
        send(&dictation()?, voice::Prewarm).await
    }

    async fn dictation_start(
        &self,
        ext: &Extensions,
        sample_rate: Option<u32>,
    ) -> RpcResult<voice::DictationId> {
        let peer = peer(ext)?;
        let source = match sample_rate {
            None if peer.remote() => {
                return Err(failed(
                    "microphone is on the harness host; pass sample_rate to stream audio",
                ));
            }
            None => voice::AudioSource::Mic,
            Some(sample_rate) => voice::AudioSource::Remote { sample_rate },
        };
        let actor = dictation()?;
        let id = send(&actor, voice::StartDictation { source }).await?;
        if source != voice::AudioSource::Mic {
            // A dropped connection must not hold the one-dictation slot.
            tokio::spawn(async move {
                peer.closed().await;
                let _ = send(&actor, voice::StopDictation { id }).await;
            });
        }
        Ok(id)
    }

    async fn dictation_audio(
        &self,
        ext: &Extensions,
        id: voice::DictationId,
        pcm: String,
    ) -> RpcResult<()> {
        peer(ext)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(pcm)
            .map_err(|e| failed(format!("bad base64: {e}")))?;
        if bytes.len() % 2 != 0 {
            return Err(failed("pcm must be little-endian i16"));
        }
        let pcm = bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        send(&dictation()?, voice::PushAudio { id, pcm }).await
    }

    async fn dictation_stop(&self, ext: &Extensions, id: voice::DictationId) -> RpcResult<()> {
        peer(ext)?;
        send(&dictation()?, voice::StopDictation { id }).await
    }

    async fn dictation_status(&self, ext: &Extensions) -> RpcResult<voice::DictationState> {
        peer(ext)?;
        send(&dictation()?, voice::DictationStatus).await
    }

    async fn meeting_processes(&self) -> RpcResult<Vec<meeting::ProcessInfo>> {
        send(&meeting()?, meeting::ListProcesses).await
    }

    async fn meeting_start(&self, start: meeting::StartMeeting) -> RpcResult<meeting::Started> {
        send(&meeting()?, start).await
    }

    async fn meeting_pause(&self) -> RpcResult<()> {
        send(&meeting()?, meeting::PauseMeeting).await
    }

    async fn meeting_resume(&self) -> RpcResult<()> {
        send(&meeting()?, meeting::ResumeMeeting).await
    }

    async fn meeting_stop(&self) -> RpcResult<()> {
        send(&meeting()?, meeting::StopMeeting).await
    }

    async fn meeting_status(&self) -> RpcResult<meeting::Status> {
        send(&meeting()?, meeting::MeetingStatus).await
    }

    async fn meeting_list(&self, limit: Option<u32>) -> RpcResult<Vec<meeting::MeetingSummary>> {
        send(&meeting()?, meeting::ListMeetings { limit }).await
    }

    async fn meeting_get(&self, id: meeting::MeetingId) -> RpcResult<meeting::Manifest> {
        send(&meeting()?, meeting::GetMeeting { meeting: id }).await
    }

    async fn meeting_delete(&self, id: meeting::MeetingId) -> RpcResult<()> {
        send(&meeting()?, meeting::DeleteMeeting { meeting: id }).await
    }

    async fn meeting_permissions(&self) -> RpcResult<meeting::Permissions> {
        send(&meeting()?, meeting::CheckPermissions).await
    }

    async fn meeting_tap(&self, topic: meeting::TopicId) -> RpcResult<()> {
        send(&meeting()?, meeting::TapTopic { topic }).await
    }

    async fn meeting_dismiss(&self, topic: meeting::TopicId) -> RpcResult<()> {
        send(&meeting()?, meeting::DismissTopic { topic }).await
    }

    async fn meeting_deeper(&self, brief: meeting::BriefId) -> RpcResult<()> {
        send(&meeting()?, meeting::Deeper { brief }).await
    }

    async fn meeting_request(&self, text: String) -> RpcResult<()> {
        send(&meeting()?, meeting::TypedRequest { text }).await
    }

    async fn meeting_ptt(&self, held: bool) -> RpcResult<()> {
        send(&meeting()?, meeting::PushToTalk { held }).await
    }

    async fn meeting_cursor(&self, brief: Option<meeting::BriefId>) -> RpcResult<()> {
        send(&meeting()?, meeting::SetCursor { brief }).await
    }

    async fn meeting_briefs(&self, id: meeting::MeetingId) -> RpcResult<Vec<meeting::Brief>> {
        send(&meeting()?, meeting::ListBriefs { meeting: id }).await
    }

    async fn meeting_topics(&self) -> RpcResult<Vec<meeting::Topic>> {
        send(&meeting()?, meeting::ListTopics).await
    }

    async fn meeting_live(&self, id: meeting::MeetingId) -> RpcResult<Vec<meeting::Utterance>> {
        send(&meeting()?, meeting::LiveText { meeting: id }).await
    }

    async fn meeting_transcript(&self, id: meeting::MeetingId) -> RpcResult<meeting::Transcript> {
        send(&meeting()?, meeting::GetTranscript { meeting: id }).await
    }

    async fn meeting_edit(
        &self,
        id: meeting::MeetingId,
        ops: Vec<meeting::EditOp>,
    ) -> RpcResult<meeting::Transcript> {
        send(&meeting()?, meeting::EditTranscript { meeting: id, ops }).await
    }

    async fn meeting_segment_audio(
        &self,
        id: meeting::MeetingId,
        track: meeting::Track,
        t0_ms: u64,
        t1_ms: u64,
    ) -> RpcResult<String> {
        send(
            &meeting()?,
            meeting::SegmentAudio {
                meeting: id,
                track,
                t0_ms,
                t1_ms,
            },
        )
        .await
    }

    async fn meeting_speakers(&self) -> RpcResult<Vec<meeting::Profile>> {
        send(&meeting()?, meeting::ListSpeakers).await
    }

    async fn meeting_oracle_mode(&self) -> RpcResult<decision::ModeInfo> {
        ask::<decision::Engine, _>(
            decision::DECISION_ACTOR,
            decision::GetMode {
                prefix: meeting::oracle::PREFIX.to_owned(),
            },
        )
        .await
    }

    async fn set_meeting_oracle_mode(&self, mode: decision::Mode) -> RpcResult<decision::ModeInfo> {
        ask::<decision::Engine, _>(
            decision::DECISION_ACTOR,
            decision::SetMode {
                prefix: meeting::oracle::PREFIX.to_owned(),
                mode,
            },
        )
        .await
    }

    async fn decision_stats(&self) -> RpcResult<Vec<decision::FamilyStats>> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::AllStats).await
    }

    async fn decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::GetTrain { prefix }).await
    }

    async fn set_decision_training(&self, prefix: String, on: bool) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::SetTrain { prefix, on }).await
    }

    async fn meeting_speaker_delete(&self, profile: meeting::ProfileId) -> RpcResult<()> {
        send(&meeting()?, meeting::DeleteSpeaker { profile }).await
    }

    async fn meeting_summarize(&self, id: meeting::MeetingId) -> RpcResult<()> {
        send(&meeting()?, meeting::Summarize { meeting: id }).await
    }

    async fn meeting_post_retry(&self, id: meeting::MeetingId) -> RpcResult<()> {
        send(&meeting()?, meeting::PostRetry { meeting: id }).await
    }

    async fn meeting_summary(&self, id: meeting::MeetingId) -> RpcResult<String> {
        send(&meeting()?, meeting::GetSummary { meeting: id }).await
    }

    async fn brief_search(&self, query: String, limit: Option<u32>) -> RpcResult<Vec<meeting::CachedBrief>> {
        send(&meeting()?, meeting::BriefSearch { query, limit }).await
    }

    async fn brief_list(&self, tag: Option<String>, limit: Option<u32>) -> RpcResult<Vec<meeting::CachedBrief>> {
        send(&meeting()?, meeting::BriefList { tag, limit }).await
    }

    async fn brief_tags(&self) -> RpcResult<Vec<String>> {
        send(&meeting()?, meeting::BriefTags).await
    }

    async fn brief_get(&self, id: meeting::BriefId) -> RpcResult<meeting::CachedBrief> {
        send(&meeting()?, meeting::BriefGet { id }).await
    }

    async fn brief_put(&self, put: meeting::CachedBriefPut) -> RpcResult<meeting::CachedBrief> {
        send(
            &meeting()?,
            meeting::BriefPut {
                put,
                source: "user".into(),
            },
        )
        .await
        .map(|(row, _)| row)
    }

    async fn brief_delete(&self, id: meeting::BriefId) -> RpcResult<()> {
        send(&meeting()?, meeting::BriefDelete { id }).await
    }

    async fn fire_schedule(&self, name: String) -> RpcResult<SessionId> {
        send(&schedule()?, schedule::Fire { name }).await
    }

    async fn enable_schedule(
        &self,
        name: String,
        enabled: bool,
    ) -> RpcResult<schedule::ScheduleInfo> {
        send(&schedule()?, schedule::SetEnabled { name, enabled }).await
    }

    async fn schedule_history(
        &self,
        name: String,
        limit: u32,
    ) -> RpcResult<Vec<schedule::RunInfo>> {
        send(&schedule()?, schedule::History { name, limit }).await
    }

    async fn list_artifacts(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<artifact::Artifact>> {
        self.authorize_opt(ext, session).await?;
        send(
            &artifact()?,
            artifact::List {
                session,
                descendants: descendants.unwrap_or(false),
            },
        )
        .await
    }

    async fn export_artifact(&self, id: String, dest: PathBuf) -> RpcResult<PathBuf> {
        send(&artifact()?, artifact::Export { id, dest }).await
    }

    async fn get_app(&self, record_call_id: String) -> RpcResult<AppDoc> {
        send(&apps()?, GetApp { record_call_id }).await
    }

    async fn app_call_tool(&self, ext: &Extensions, args: CallArgs) -> RpcResult<AppToolResult> {
        let peer = peer(ext)?;
        send(&apps()?, CallTool { peer, args }).await
    }

    async fn app_call_status(
        &self,
        ext: &Extensions,
        request_id: String,
    ) -> RpcResult<AppToolResult> {
        let peer = peer(ext)?;
        send(&apps()?, CallStatusOf { peer, request_id }).await
    }

    async fn app_close(&self, ext: &Extensions, instance_id: String) -> RpcResult<()> {
        let peer = peer(ext)?;
        send(&apps()?, CloseApp { peer, instance_id }).await
    }

    async fn get_artifact(&self, call_id: String) -> RpcResult<artifact::Artifact> {
        send(&artifact()?, artifact::search::Get { call_id }).await
    }

    async fn artifact_search_match(
        &self,
        call_id: String,
        query: String,
    ) -> RpcResult<ArtifactMatchContext> {
        send(
            &artifact()?,
            artifact::search::ReadSearchMatch { call_id, query },
        )
        .await
    }

    async fn list_secrets(&self) -> RpcResult<secrets::SecretsList> {
        send(&secrets()?, secrets::List).await
    }

    async fn set_secret(
        &self,
        name: String,
        value: String,
        note: Option<String>,
    ) -> RpcResult<secrets::SecretInfo> {
        send(
            &secrets()?,
            secrets::Set {
                name,
                value,
                note,
                source: "user".into(),
            },
        )
        .await
    }

    async fn restore_secret(&self, name: String, back: u8) -> RpcResult<secrets::SecretInfo> {
        send(&secrets()?, secrets::Restore { name, back }).await
    }

    async fn delete_secret(&self, name: String) -> RpcResult<()> {
        send(&secrets()?, secrets::Delete { name }).await
    }

    async fn answer_secret(&self, id: secrets::RequestId, value: Option<String>) -> RpcResult<()> {
        send(&secrets()?, secrets::Answer { id, value })
            .await
            .map(|_| ())
    }

    async fn put_attachment(&self, bytes: String, ext: String) -> RpcResult<PathBuf> {
        let bytes = decode_attachment(bytes, &ext)?;
        save_blob(&bytes, &ext, &self.rt.data_dir).map_err(|e| failed(e.to_string()))
    }

    async fn get_attachment(
        &self,
        ext: &Extensions,
        path: PathBuf,
        session: Option<SessionId>,
    ) -> RpcResult<String> {
        // A private tree's files live under its scratch root, never under
        // `data_dir`: without the owning session they are not served at all.
        let root = match session {
            Some(s) => self.authorize(ext, s).await?.resources.root.clone(),
            None => self.rt.data_dir.clone(),
        };
        let real =
            std::fs::canonicalize(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let allowed = ["attachments", "browser", "computer", "artifacts"]
            .iter()
            .any(|d| std::fs::canonicalize(root.join(d)).is_ok_and(|dir| real.starts_with(dir)));
        if !allowed {
            return Err(failed(format!("{}: not an attachment", path.display())));
        }
        let bytes = tokio::fs::read(&real)
            .await
            .map_err(|e| failed(format!("{}: {e}", path.display())))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    async fn subscribe_events(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult {
        // Subscribe before the snapshot and before accept so nothing published
        // in between is missed; a child in both the snapshot and the feed is
        // inserted twice, harmlessly.
        let mut rx = self.events.subscribe();
        // Admission first; a refusal is answered as the call's error (the
        // peer gate's `-32002`), not jsonrpsee's anonymous internal error.
        let admit = async {
            let peer = peer(ext)?.id();
            // Private trees opened elsewhere (another process's server, before
            // this map existed) become known here; the filter fails closed on
            // every member for everyone but its owner (51 §4.2).
            if let Ok(trees) = ask::<Agents, _>(agent::AGENT_ACTOR, PrivateTrees).await {
                self.private.write().extend(
                    trees
                        .into_iter()
                        .map(|(member, root, owner)| (member, (root, owner))),
                );
            }
            let tree: Option<HashSet<String>> = match (session, descendants) {
                (Some(root), Some(true)) => {
                    let mut set: HashSet<String> = self
                        .list_agents(ext, root)
                        .await?
                        .into_iter()
                        .map(|a| a.id.to_string())
                        .collect();
                    set.insert(root.to_string());
                    Some(set)
                }
                (Some(root), _) => {
                    self.authorize(ext, root).await?;
                    None
                }
                _ => None,
            };
            RpcResult::Ok((peer, tree))
        };
        let (peer, mut tree) = match admit.await {
            Ok(v) => v,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        let session = session.map(|s| s.to_string());
        let private = self.private.clone();
        // Replay in-flight asks so a client that subscribes (or reconnects over
        // the relay) *after* the AskRequested broadcast still gets the card,
        // rather than a card that never arrives and a tool that blocks until
        // its human timeout (31 §7.3).
        if let Ok(Some(actor)) = ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR)
            && let Ok(asks) = actor.ask(tools::PendingAsks).await
        {
            for a in asks {
                let Ok(v) = serde_json::to_value(&a) else {
                    continue;
                };
                if !wanted(&v, session.as_deref(), tree.as_mut()) || !admitted(&v, peer, &private) {
                    continue;
                }
                if let Ok(raw) = serde_json::value::to_raw_value(&v) {
                    let _ = sink.send(SubscriptionMessage::from(raw)).await;
                }
            }
        }
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    _ = sink.closed() => return,
                    r = rx.recv() => match r {
                        Ok(ev) => {
                            if !wanted(&ev, session.as_deref(), tree.as_mut())
                                || !admitted(&ev, peer, &private)
                            {
                                continue;
                            }
                            serde_json::value::to_raw_value(&*ev)
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            serde_json::value::to_raw_value(&serde_json::json!({ "type": "Lagged", "count": n }))
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                };
                let Ok(raw) = msg else { continue };
                if sink.send(SubscriptionMessage::from(raw)).await.is_err() {
                    return;
                }
            }
        });
        Ok(())
    }
}

/// Does `peer` get `ev` past private ownership (51 §4.2)? An event naming a
/// private member in `session`, `parent` or `child` reaches only that tree's
/// owner; a `SubagentSpawned` under a private parent enrols `child` first so
/// its own later events are filtered too. Events naming no known member pass:
/// durable trees are not tracked.
fn admitted(ev: &serde_json::Value, peer: u64, private: &PrivateMap) -> bool {
    let id = |k: &str| {
        ev.get(k)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<SessionId>().ok())
    };
    if ev.get("type").and_then(|t| t.as_str()) == Some("SubagentSpawned")
        && let (Some(parent), Some(child)) = (id("parent"), id("child"))
    {
        let known = private.read().get(&parent).copied();
        if let Some((root, owner)) = known {
            private.write().insert(child, (root, owner));
            return owner == peer;
        }
    }
    let map = private.read();
    ["session", "parent", "child"]
        .into_iter()
        .filter_map(id)
        .all(|s| map.get(&s).is_none_or(|&(_, owner)| owner == peer))
}

/// Does a subscription on `session` forward `ev`? With `tree` (descendants
/// mode) an event passes when its `session` is in the tree, or it is a
/// `SubagentSpawned` whose `parent` is — then `child` joins the tree.
/// Without, everything passes unless both sides name a different session.
/// `CommunicationChanged` always passes: a global or ancestor switch changes
/// every descendant's effective state, the filter cannot see ancestry, and
/// the event carries only a session id and a revision (50 §10).
fn wanted(
    ev: &serde_json::Value,
    session: Option<&str>,
    tree: Option<&mut HashSet<String>>,
) -> bool {
    let field = |k: &str| ev.get(k).and_then(|v| v.as_str());
    if field("type") == Some("CommunicationChanged") {
        return true;
    }
    let Some(tree) = tree else {
        return match (session, field("session")) {
            (Some(want), Some(have)) => want == have,
            _ => true,
        };
    };
    if field("session").is_some_and(|s| tree.contains(s)) {
        return true;
    }
    if field("type") == Some("SubagentSpawned")
        && field("parent").is_some_and(|p| tree.contains(p))
        && let Some(child) = field("child")
    {
        tree.insert(child.to_owned());
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn communication_changed_reaches_scoped_and_tree_subscribers() {
        let global =
            serde_json::json!({ "type": "CommunicationChanged", "session": null, "revision": 3 });
        let root =
            serde_json::json!({ "type": "CommunicationChanged", "session": "r", "revision": 4 });
        let other = serde_json::json!({ "type": "TitleChanged", "session": "r", "title": "x" });
        let mut tree: HashSet<String> = ["c".to_owned()].into();
        assert!(wanted(&global, Some("c"), None));
        assert!(wanted(&root, Some("c"), None));
        assert!(wanted(&global, Some("c"), Some(&mut tree)));
        assert!(wanted(&root, Some("c"), Some(&mut tree)));
        assert!(!wanted(&other, Some("c"), None));
        assert!(!wanted(&other, Some("c"), Some(&mut tree)));
    }

    /// Private events reach their owner only (51 §4.2): a member's event is
    /// dropped for every other peer; `SubagentSpawned` under a private parent
    /// enrols the child so its own events are filtered too; events naming no
    /// private member pass for everyone.
    #[test]
    fn private_events_reach_only_their_owner() {
        let (p, c, d) = (SessionId::new(), SessionId::new(), SessionId::new());
        let private: PrivateMap = Default::default();
        private.write().insert(p, (p, 7));
        let title = serde_json::json!({ "type": "TitleChanged", "session": p, "title": "x" });
        assert!(admitted(&title, 7, &private));
        assert!(!admitted(&title, 8, &private));

        let spawned = serde_json::json!({ "type": "SubagentSpawned", "parent": p, "child": c });
        assert!(!admitted(&spawned, 8, &private));
        assert!(admitted(&spawned, 7, &private));
        assert_eq!(private.read().get(&c), Some(&(p, 7)));
        let child = serde_json::json!({ "type": "TextDelta", "session": c, "text": "hi" });
        assert!(admitted(&child, 7, &private));
        assert!(!admitted(&child, 8, &private));

        let durable = serde_json::json!({ "type": "TitleChanged", "session": d, "title": "y" });
        let none = serde_json::json!({ "type": "Lagged", "count": 1 });
        for peer in [7, 8] {
            assert!(admitted(&durable, peer, &private));
            assert!(admitted(&none, peer, &private));
        }
    }

    /// Reads and mutations on a session need an authenticated connection
    /// before anything else is consulted; a subscription is no exception.
    #[tokio::test]
    async fn session_calls_need_an_authenticated_peer() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
        let id = SessionId::new();
        for req in [
            format!(r#"{{"jsonrpc":"2.0","method":"get_todos","params":["{id}"],"id":1}}"#),
            format!(r#"{{"jsonrpc":"2.0","method":"session_cost","params":["{id}"],"id":1}}"#),
            format!(
                r#"{{"jsonrpc":"2.0","method":"put_session_attachment","params":["{id}","AA==","png"],"id":1}}"#
            ),
            r#"{"jsonrpc":"2.0","method":"create_session","params":[{}],"id":1}"#.to_owned(),
            r#"{"jsonrpc":"2.0","method":"subscribe_events","params":[null,null],"id":1}"#
                .to_owned(),
        ] {
            let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
            let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(i64::from(UNAUTHORIZED)),
                "{req}"
            );
        }
    }

    /// The JSON-RPC error code of a raw `app_close` on `methods`.
    async fn app_close_code(methods: &jsonrpsee::Methods) -> i64 {
        let (resp, _) = methods
            .raw_json_request(
                r#"{"jsonrpc":"2.0","method":"app_close","params":["v1"],"id":1}"#,
                1,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        v["error"]["code"].as_i64().expect("error reply")
    }

    /// App calls are scoped to the connection that made them: a dispatch with
    /// no `RpcPeer` in its extensions is unauthorized, never treated as local
    /// (46 §6.3). With a peer the call reaches the actor lookup.
    #[tokio::test]
    async fn app_calls_need_an_authenticated_peer() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let base: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
        assert_eq!(app_close_code(&base).await, i64::from(UNAUTHORIZED));

        let (peer, _guard) = link::RpcPeer::open();
        let mut with_peer = base.clone();
        with_peer.extensions_mut().insert(peer);
        // No app actor in this process: past the peer gate, the lookup fails.
        assert_eq!(app_close_code(&with_peer).await, i64::from(NOT_RUNNING));
    }

    /// Dispatch a raw dictation request as a remote (relay) peer on a fresh server.
    async fn remote_dictation_call(method: &str, params: &str) -> serde_json::Value {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
        let (peer, _guard) = link::RpcPeer::open_remote();
        methods.extensions_mut().insert(peer);
        let req = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params},"id":1}}"#);
        let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
        serde_json::from_str(resp.get()).unwrap()
    }

    /// The daemon's microphone is out of a relay peer's reach: `dictation_start`
    /// without a `sample_rate` is refused with the reason, before any actor
    /// lookup (54 §8).
    #[tokio::test]
    async fn dictation_mic_refuses_relay_peers() {
        let v = remote_dictation_call("dictation_start", "[null]").await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(ASK_FAILED)));
        assert_eq!(
            v["error"]["message"].as_str(),
            Some("microphone is on the harness host; pass sample_rate to stream audio")
        );
    }

    /// Everything but the daemon mic is open to a relay peer: `dictation_status`
    /// reaches the actor lookup (no voice actor here, so `NOT_RUNNING`).
    #[tokio::test]
    async fn dictation_status_reaches_relay_peers() {
        let v = remote_dictation_call("dictation_status", "[]").await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(NOT_RUNNING)));
    }
}
