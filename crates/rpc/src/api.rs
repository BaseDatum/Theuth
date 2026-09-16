//! The JSON-RPC surface. Every method is a lazy registry lookup plus one
//! `ask`; the subscription drains a `broadcast::Receiver` fed by
//! [`crate::bridge`].

use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use agent::{
    AddAcp, AddProvider, AddSkill, AddSkillRepo, AgentConfig, AgentInfo, Agents, Cancel, Compact,
    CompactStyle, DeleteSession, EditTurn, ForkSession, GetCompactStyle, GetSubagentTimeout,
    GetTitleModel, Harvest, ListAcp, ListAgents, ListLogins, ListModels, ListProviders, ListQueued,
    ListRegistry, ListSessions, ListSkills, LoadSession, LoadSkill, Loaded, Login, LoginInfo,
    Logout, ModelInfo, NewSession, ProviderInfo, QueuedInput, Redo, Regenerate, RemoveAcp,
    RemoveProvider, RemoveSkill, RemoveSkillRepo, RenameSession, SearchSessions, SendInput,
    SessionSummary, SetCompactStyle, SetEffort, SetModel, SetSubagentTimeout, SetTitleModel,
    ShowPrompt, SkillBody, SkillMeta, Steer, SyncSkills, Undo, UseSkill, Withdraw,
};
use base64::Engine as _;
use browser::{BROWSER_ACTOR, Browsers, Screenshot};
use computer::{COMPUTER_ACTOR, Computer, Permissions};
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::types::ErrorObjectOwned;
use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::error::SendError;
use kameo::message::Message;
use kameo::reply::Reply;
use knowledge::{
    Change, Concept, Delete, Get, HarvestResult, History, HistoryOf, Hit, KNOWLEDGE_ACTOR,
    KnowledgeActor, List, Meta, Put, PutConcept, Restore, Search, Source, SyncNow, Write,
};
use link::{
    CancelPair, Enable, LINK_ACTOR, LinkActor, LinkConfig, LinkStatus, Pair, Pairing, Rename,
    RenameDevice, Revoke,
};
use runtime::{InputId, Runtime, SessionId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

use crate::bridge::Event;

/// `ask` failed (handler error, actor stopped mid-call).
const ASK_FAILED: i32 = -32000;
/// The named actor is not registered.
const NOT_RUNNING: i32 = -32001;

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
    /// Harness display name (`link.name`, else the hostname): what a linked
    /// device shows in its harness switcher (31 §6.4).
    pub name: String,
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
    /// the last `limit` of them; both omitted = everything.
    #[method(name = "load_session")]
    async fn load_session(
        &self,
        session: SessionId,
        before: Option<i64>,
        limit: Option<u32>,
    ) -> RpcResult<Loaded>;
    /// Set a session's title by hand; `TitleChanged` follows on the stream.
    #[method(name = "rename_session")]
    async fn rename_session(&self, session: SessionId, title: String) -> RpcResult<()>;
    /// Delete a session and its subagents for good; a `SessionDeleted` per removed session
    /// follows on the stream. Refused while any of them is mid-turn.
    #[method(name = "delete_session")]
    async fn delete_session(&self, session: SessionId) -> RpcResult<()>;
    /// Fuzzy search (nucleo, design doc 26) over every session's title and its
    /// user/assistant messages: the best hit per session, best score first, with a
    /// snippet and the matched char ranges. Empty query → `[]`. `limit` defaults to 30, max 50.
    #[method(name = "search_sessions")]
    async fn search_sessions(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<agent::search::SearchHit>>;
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
    /// and return the path, for `send_input { files }`. `ext` ∈ png, jpg,
    /// jpeg, gif, webp, pdf; images ≤ 5 MB, PDFs ≤ 32 MB decoded.
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
/// file is not rewritten. Shared by `put_attachment` and the ACP prompt path.
pub(crate) fn save_blob(bytes: &[u8], ext: &str, data_dir: &Path) -> std::io::Result<PathBuf> {
    let hex = format!("{:x}", Sha256::digest(bytes));
    let dir = data_dir.join("attachments");
    let path = dir.join(format!("{}.{ext}", &hex[..16]));
    if !path.exists() {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, bytes)?;
    }
    Ok(path)
}

/// An `ASK_FAILED` (`-32000`) error with `msg`.
fn failed(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(ASK_FAILED, msg.into(), None::<()>)
}

/// Largest decoded image `put_attachment` accepts (17 §1: Anthropic's 5 MB).
const MAX_IMAGE: usize = 5 * 1024 * 1024;
/// Largest decoded PDF `put_attachment` accepts (20 §2.5: Anthropic's 32 MB).
const MAX_PDF: usize = 32 * 1024 * 1024;

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
            name,
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
    ) -> RpcResult<Loaded> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            LoadSession {
                session,
                before,
                limit,
            },
        )
        .await
    }

    async fn rename_session(&self, session: SessionId, title: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RenameSession { session, title }).await
    }

    async fn delete_session(&self, session: SessionId) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, DeleteSession { session }).await
    }

    async fn search_sessions(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<agent::search::SearchHit>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SearchSessions { query, limit }).await
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

    async fn put_attachment(&self, bytes: String, ext: String) -> RpcResult<PathBuf> {
        let (ext, cap) = match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" => (ext.as_str(), MAX_IMAGE),
            "pdf" => ("pdf", MAX_PDF),
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
        let allowed = ["attachments", "browser", "computer"].iter().any(|d| {
            std::fs::canonicalize(self.rt.data_dir.join(d)).is_ok_and(|dir| real.starts_with(dir))
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
