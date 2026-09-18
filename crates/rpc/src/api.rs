//! The JSON-RPC surface. Every method is a lazy registry lookup plus one
//! `ask`; the subscription drains a `broadcast::Receiver` fed by
//! [`crate::bridge`].

use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use ::search::SearchCategory;
use agent::todo::TodoSnapshot;
use agent::{
    AddAcp, AddProvider, AddSkill, AddSkillRepo, AgentConfig, AgentInfo, Agents, Cancel, Compact,
    CompactStyle, ContextItems, ContextRemoval, ContextSnapshot, ContextTarget, DeleteSession,
    EditTurn, ForkSession, GetCompactStyle, GetMaxToolRounds, GetSubagentTimeout, GetTitleModel,
    GetTodos, Harvest, ListAcp, ListAgents, ListLogins, ListModels, ListProviders, ListQueued,
    ListRegistry, ListSessions, ListSkills, LoadSession, LoadSkill, Loaded, Login, LoginInfo,
    Logout, ModelInfo, NewSession, PinSession, ProviderInfo, QueuedInput, Redo, Regenerate,
    RemoveAcp, RemoveContext, RemoveProvider, RemoveSkill, RemoveSkillRepo, RenameSession,
    SendInput, SessionSummary, SetCompactStyle, SetEffort, SetMaxToolRounds, SetModel,
    SetSubagentTimeout, SetTitleModel, ShowPrompt, SkillBody, SkillMeta, Steer, SyncSkills, Undo,
    UseSkill, Withdraw,
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
use runtime::{InputId, Runtime, SessionId};
use serde::{Deserialize, Serialize};
use signal::{Allow, Link, RemoveAllowed, SIGNAL_ACTOR, Signal, SignalStatus, Status, Unlink};
use slack::{SLACK_ACTOR, Slack, SlackStatus};
use tokio::sync::broadcast;
use tools::git::SyncResult;
use tools::{
    AddRepo, AddServer, Answer, Approval, ApprovalId, ApprovalsState, AskId, CancelJob, Decision,
    Gate, GetApprovals, JOBS_ACTOR, JobId, JobView, Jobs, ListJobs, ListServers, ListTools, Mcp,
    McpLogin, McpLogout, McpProgress, McpServer, Progress, RemoveServer, RepoInfo, Resolve,
    SetApprovals, SetGate, SetHidden, SyncMcp, ToolRegistry, ToolSpec,
};
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
    /// Harness display name (`link.name`, else the hostname): what a linked
    /// device shows in its harness switcher (31 §6.4).
    pub name: String,
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
    /// Create an idle session (no turn) so per-session state can be set before
    /// the first message; `send_input` to it afterwards.
    #[method(name = "new_session")]
    async fn new_session(&self) -> RpcResult<SessionId>;
    /// Start a turn; `session: None` creates a session. `files` are image files
    /// (17 §2) attached to the message. Returns once queued.
    #[method(name = "send_input")]
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<SessionId>;
    /// Queue a user message after the current tool batch; returns the id (minted
    /// when omitted). `files` are image files (17 §2).
    #[method(name = "steer")]
    async fn steer(
        &self,
        session: SessionId,
        text: String,
        id: Option<InputId>,
        files: Vec<PathBuf>,
    ) -> RpcResult<InputId>;
    /// Pull a queued message back out; returns its text.
    #[method(name = "withdraw")]
    async fn withdraw(&self, session: SessionId, id: InputId) -> RpcResult<String>;
    /// Messages waiting for the current tool batch to end.
    #[method(name = "list_queued")]
    async fn list_queued(&self, session: SessionId) -> RpcResult<Vec<QueuedInput>>;
    /// Abort the in-flight turn.
    #[method(name = "cancel")]
    async fn cancel(&self, session: SessionId) -> RpcResult<()>;
    /// Background jobs of a session (14 §2): id, state, seconds, log path, tail.
    #[method(name = "list_jobs")]
    async fn list_jobs(&self, session: SessionId) -> RpcResult<Vec<JobView>>;
    /// Kill a running job; `Err` when unknown or already finished.
    #[method(name = "cancel_job")]
    async fn cancel_job(&self, session: SessionId, id: JobId) -> RpcResult<()>;
    /// Current page of `session`'s embedded browser as a base64 PNG for a live
    /// view (23 §3.2); `None` when the session has no live isolate or no open
    /// page. Polling never spawns an isolate.
    #[method(name = "browser_screenshot")]
    async fn browser_screenshot(&self, session: SessionId) -> RpcResult<Option<String>>;
    /// Computer-control permissions and attached displays (design doc 27 §6);
    /// errors with "actor not running" when `computer.enabled` is false.
    #[method(name = "computer_permissions")]
    async fn computer_permissions(&self) -> RpcResult<computer::PermissionStatus>;
    /// Fold the conversation now (design doc 07 §8.6 / 11 §4.2); returns the
    /// new epoch. `style: None` uses the configured default. Refused while a
    /// turn is running.
    #[method(name = "compact")]
    async fn compact(&self, session: SessionId, style: Option<CompactStyle>) -> RpcResult<u32>;
    /// Hide the last turn from the frame; returns its first line.
    #[method(name = "undo")]
    async fn undo(&self, session: SessionId) -> RpcResult<String>;
    /// Restore the last undone turn; returns its first line.
    #[method(name = "redo")]
    async fn redo(&self, session: SessionId) -> RpcResult<String>;
    /// Inventory of the active context (design doc 48 §3.2): revision,
    /// editability, and every removable message/binary/head with its
    /// protocol companions. No bodies. Native sessions only.
    #[method(name = "context_items")]
    async fn context_items(&self, session: SessionId) -> RpcResult<ContextSnapshot>;
    /// Remove `targets` from the active context at `expected_revision`
    /// (design doc 48 §3.2). Atomic: stale revision, unknown targets or an
    /// incomplete protocol group refuse and write nothing. Refused while a
    /// turn is running; a `ContextChanged` follows on the stream.
    #[method(name = "remove_context")]
    async fn remove_context(
        &self,
        session: SessionId,
        expected_revision: String,
        targets: Vec<ContextTarget>,
    ) -> RpcResult<ContextRemoval>;
    /// Regenerate the reply to the turn opened at `turn` (a `HistoryEntry`
    /// `turn` handle): drop it and everything after, replay the stored opener
    /// (design doc 29 §3). Refused while a turn is running.
    #[method(name = "regenerate")]
    async fn regenerate(&self, session: SessionId, turn: i64) -> RpcResult<()>;
    /// Replace the turn opened at `turn` with `text` (+ `files`): drop it and
    /// everything after, start a turn with the new input (design doc 29 §3).
    /// Refused while a turn is running.
    #[method(name = "edit_turn")]
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
    #[method(name = "fork_session")]
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
    /// The live title model (`agent.title_model`), if set.
    #[method(name = "title_model")]
    async fn title_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `agent.title_model`; `None` clears it (titles use the session's model).
    #[method(name = "set_title_model")]
    async fn set_title_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// Every session, newest first.
    #[method(name = "list_sessions")]
    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>>;
    /// The subtree under `session` (children, grandchildren, …), pre-order,
    /// each with its live state (19 §2.1).
    #[method(name = "list_agents")]
    async fn list_agents(&self, session: SessionId) -> RpcResult<Vec<AgentInfo>>;
    /// Rendered history of one session plus whether a turn is in flight.
    /// `before`/`limit` page the entries (25 §7): those with `seq < before`,
    /// the last `limit` of them; both omitted = everything. `settled_only`
    /// (48 §5) reads through the idle session actor; while busy the reply
    /// has `settled: false` and `context_editable: false` with a reason.
    #[method(name = "load_session")]
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
    #[method(name = "get_todos")]
    async fn get_todos(&self, session: SessionId) -> RpcResult<TodoSnapshot>;
    /// Set a session's title by hand; `TitleChanged` follows on the stream.
    #[method(name = "rename_session")]
    async fn rename_session(&self, session: SessionId, title: String) -> RpcResult<()>;
    /// Pin or unpin a session; pinned sessions lead `list_sessions`.
    #[method(name = "pin_session")]
    async fn pin_session(&self, session: SessionId, pinned: bool) -> RpcResult<()>;
    /// Delete a session and its subagents for good; a `SessionDeleted` per removed session
    /// follows on the stream. Refused while any of them is mid-turn.
    #[method(name = "delete_session")]
    async fn delete_session(&self, session: SessionId) -> RpcResult<()>;
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
    #[method(name = "set_model")]
    async fn set_model(&self, session: Option<SessionId>, model: String) -> RpcResult<ModelInfo>;
    /// Set (or clear with `None`) the reasoning effort for `session` from its next turn, and as
    /// the default (`agent.effort`). Values come from the model's catalog entry
    /// (`ModelInfo.efforts`, or an integer token budget within `budget_min..budget_max`);
    /// `EffortChanged` follows on the stream.
    #[method(name = "set_effort")]
    async fn set_effort(&self, session: Option<SessionId>, effort: Option<String>)
    -> RpcResult<()>;
    /// MCP servers, their tools, and their `mcp/config.yaml` risk lines; with
    /// a session, which tools it hid (`set_mcp_hidden`).
    #[method(name = "list_mcp")]
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
    #[method(name = "set_mcp_hidden")]
    async fn set_mcp_hidden(
        &self,
        session: SessionId,
        tools: Vec<String>,
        hidden: bool,
    ) -> RpcResult<()>;
    /// Turn approval prompts on/off for `session`, or globally (persisted) with `None`.
    #[method(name = "set_approvals")]
    async fn set_approvals(
        &self,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<ApprovalsState>;
    /// The assembled system prompt; without `session` the peers section is omitted.
    #[method(name = "get_prompt")]
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
    #[method(name = "use_skill")]
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
    #[method(name = "knowledge_harvest")]
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
    /// paths under `<data_dir>/{attachments,browser,computer}` (after
    /// canonicalization) are served.
    #[method(name = "get_attachment")]
    async fn get_attachment(&self, path: PathBuf) -> RpcResult<String>;
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
    #[method(name = "speak")]
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
    #[method(name = "list_artifacts")]
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
    #[subscription(name = "subscribe_events", unsubscribe = "unsubscribe_events", item = serde_json::Value)]
    async fn subscribe_events(
        &self,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult;
}

/// Server state: the event feed and the runtime (for `shutdown`).
pub struct Server {
    /// Feed from [`crate::bridge`].
    pub events: broadcast::Sender<Event>,
    /// `rt.shutdown` is cancelled by `shutdown`.
    pub rt: Runtime,
}

impl Server {
    /// A server with no event feed: for in-process callers (the TUI), which
    /// subscribe to the bus themselves. `serve` builds the real one.
    pub fn new(rt: Runtime) -> Self {
        Self {
            events: broadcast::channel(1).0,
            rt,
        }
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

/// The link actor; absent when the plugin is not in this process's list.
fn link() -> RpcResult<ActorRef<LinkActor>> {
    lookup::<LinkActor>(LINK_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "link is disabled", None::<()>),
        _ => e,
    })
}

/// This machine's hostname, read once; `"wth"` when `hostname` fails.
static HOSTNAME: LazyLock<String> = LazyLock::new(|| {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "wth".to_owned())
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
        let name = match self.rt.section::<LinkConfig>("link") {
            Ok(c) if !c.name.is_empty() => c.name,
            _ => HOSTNAME.clone(),
        };
        Ok(Info {
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            data_dir: self.rt.data_dir.display().to_string(),
            default_model,
            approvals,
            compact_style,
            subagent_timeout_secs,
            max_tool_rounds,
            name,
            app_enabled: apps().is_ok(),
        })
    }

    async fn new_session(&self) -> RpcResult<SessionId> {
        ask::<Agents, _>(agent::AGENT_ACTOR, NewSession::default()).await
    }

    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<SessionId> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SendInput {
                session,
                text,
                files,
            },
        )
        .await
    }

    async fn steer(
        &self,
        session: SessionId,
        text: String,
        id: Option<InputId>,
        files: Vec<PathBuf>,
    ) -> RpcResult<InputId> {
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

    async fn withdraw(&self, session: SessionId, id: InputId) -> RpcResult<String> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Withdraw { session, id }).await
    }

    async fn list_queued(&self, session: SessionId) -> RpcResult<Vec<QueuedInput>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListQueued { session }).await
    }

    async fn cancel(&self, session: SessionId) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Cancel { session }).await
    }

    async fn list_jobs(&self, session: SessionId) -> RpcResult<Vec<JobView>> {
        ask::<Jobs, _>(JOBS_ACTOR, ListJobs(session)).await
    }

    async fn cancel_job(&self, session: SessionId, id: JobId) -> RpcResult<()> {
        ask::<Jobs, _>(JOBS_ACTOR, CancelJob { session, id }).await
    }

    async fn browser_screenshot(&self, session: SessionId) -> RpcResult<Option<String>> {
        ask::<Browsers, _>(BROWSER_ACTOR, Screenshot { session }).await
    }

    async fn computer_permissions(&self) -> RpcResult<computer::PermissionStatus> {
        ask::<Computer, _>(COMPUTER_ACTOR, Permissions).await
    }

    async fn compact(&self, session: SessionId, style: Option<CompactStyle>) -> RpcResult<u32> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Compact { session, style }).await
    }

    async fn undo(&self, session: SessionId) -> RpcResult<String> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Undo { session }).await
    }

    async fn redo(&self, session: SessionId) -> RpcResult<String> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Redo { session }).await
    }

    async fn context_items(&self, session: SessionId) -> RpcResult<ContextSnapshot> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ContextItems { session }).await
    }

    async fn remove_context(
        &self,
        session: SessionId,
        expected_revision: String,
        targets: Vec<ContextTarget>,
    ) -> RpcResult<ContextRemoval> {
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

    async fn regenerate(&self, session: SessionId, turn: i64) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Regenerate { session, turn }).await
    }

    async fn edit_turn(
        &self,
        session: SessionId,
        turn: i64,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<()> {
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
        session: SessionId,
        through: Option<i64>,
        reply: Option<u32>,
    ) -> RpcResult<SessionId> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ForkSession {
                session,
                through,
                reply,
            },
        )
        .await
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

    async fn title_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetTitleModel).await
    }

    async fn set_title_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetTitleModel { model }).await
    }

    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions).await
    }

    async fn list_agents(&self, session: SessionId) -> RpcResult<Vec<AgentInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListAgents { session }).await
    }

    async fn load_session(
        &self,
        session: SessionId,
        before: Option<i64>,
        limit: Option<u32>,
        settled_only: Option<bool>,
    ) -> RpcResult<Loaded> {
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

    async fn get_todos(&self, session: SessionId) -> RpcResult<TodoSnapshot> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetTodos { session }).await
    }

    async fn rename_session(&self, session: SessionId, title: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RenameSession { session, title }).await
    }

    async fn pin_session(&self, session: SessionId, pinned: bool) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, PinSession { session, pinned }).await
    }

    async fn delete_session(&self, session: SessionId) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, DeleteSession { session }).await
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

    async fn set_model(&self, session: Option<SessionId>, model: String) -> RpcResult<ModelInfo> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetModel { session, model }).await
    }

    async fn set_effort(
        &self,
        session: Option<SessionId>,
        effort: Option<String>,
    ) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetEffort { session, effort }).await
    }

    async fn list_mcp(&self, session: Option<SessionId>) -> RpcResult<Vec<McpServer>> {
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
        session: SessionId,
        tools: Vec<String>,
        hidden: bool,
    ) -> RpcResult<()> {
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
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<ApprovalsState> {
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, SetApprovals { session, enabled }).await
    }

    async fn get_prompt(&self, session: Option<SessionId>) -> RpcResult<String> {
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
        session: Option<SessionId>,
        name: String,
        args: String,
    ) -> RpcResult<SessionId> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            UseSkill {
                session,
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

    async fn knowledge_harvest(&self, session: SessionId) -> RpcResult<HarvestResult> {
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

    async fn speak(&self, session: SessionId, text: String) -> RpcResult<SpeechId> {
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
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<artifact::Artifact>> {
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
        let (ext, cap) = match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" => (ext.as_str(), MAX_IMAGE),
            "pdf" => ("pdf", MAX_PDF),
            "m4a" | "mp4" | "webm" | "ogg" | "opus" | "mp3" | "wav" => (ext.as_str(), MAX_AUDIO),
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
        save_blob(&bytes, ext, &self.rt.data_dir).map_err(|e| failed(e.to_string()))
    }

    async fn get_attachment(&self, path: PathBuf) -> RpcResult<String> {
        let real =
            std::fs::canonicalize(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let allowed = ["attachments", "browser", "computer", "artifacts"]
            .iter()
            .any(|d| {
                std::fs::canonicalize(self.rt.data_dir.join(d))
                    .is_ok_and(|dir| real.starts_with(dir))
            });
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
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult {
        // Subscribe before the snapshot and before accept so nothing published
        // in between is missed; a child in both the snapshot and the feed is
        // inserted twice, harmlessly.
        let mut rx = self.events.subscribe();
        let mut tree: Option<HashSet<String>> = match (session, descendants) {
            (Some(root), Some(true)) => {
                let mut set: HashSet<String> = self
                    .list_agents(root)
                    .await?
                    .into_iter()
                    .map(|a| a.id.to_string())
                    .collect();
                set.insert(root.to_string());
                Some(set)
            }
            _ => None,
        };
        let sink = pending.accept().await?;
        let session = session.map(|s| s.to_string());
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
                if !wanted(&v, session.as_deref(), tree.as_mut()) {
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
                            if !wanted(&ev, session.as_deref(), tree.as_mut()) {
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

/// Does a subscription on `session` forward `ev`? With `tree` (descendants
/// mode) an event passes when its `session` is in the tree, or it is a
/// `SubagentSpawned` whose `parent` is — then `child` joins the tree.
/// Without, everything passes unless both sides name a different session.
fn wanted(
    ev: &serde_json::Value,
    session: Option<&str>,
    tree: Option<&mut HashSet<String>>,
) -> bool {
    let field = |k: &str| ev.get(k).and_then(|v| v.as_str());
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
}
