//! The JSON-RPC surface. Every method is a lazy registry lookup plus one
//! `ask`; the subscription drains a `broadcast::Receiver` fed by
//! [`crate::bridge`].

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use ::search::SearchCategory;
use a2a::{A2A_ACTOR, A2aActor, A2aStatus};
use agent::supervisor::{HarnessOptions, ResumeChild, SetHarnessOption};
use agent::todo::TodoSnapshot;
use agent::{
    AddAcp, AddProvider, AddSkill, AddSkillRepo, AgentInfo, Agents, AnswerPeerRequest,
    ArchiveSession, Broadcast, BroadcastOutcome, BroadcastReceipt, BroadcastRequest,
    BroadcastTargets, Cancel, CancelScope, CommunicationState, Compact, CompactStyle,
    ContextBreakdown, ContextItems, ContextRemoval, ContextSnapshot, ContextTarget, DeleteSession,
    Delivery, EditTurn, EscalateModel, ForkSession, GetCommunication, GetCompactStyle,
    GetDefaultEffort, GetDefaultModel, GetEscalateModel, GetMaxSalvages, GetMaxToolRounds,
    GetPeerMessage, GetStreamIdle, GetSubagentModel, GetSubagentTimeout, GetTitleModel, GetTodos,
    GetVisionModel, Harvest, ListAcp, ListActivity, ListAgents, ListLogins, ListModels,
    ListPeerMessages, ListPeers, ListProviders, ListQueued, ListRegistry, ListSessions, ListSkills,
    LoadSession, LoadSkill, Loaded, Login, LoginInfo, Logout, ModelInfo, NewRootSession,
    PeerMessagePage, PeerPage, PeerReceipt, PeerRecord, PeerScope, PinSession, ProviderInfo,
    QueuedInput, Redo, Regenerate, RemoveAcp, RemoveContext, RemoveProvider, RemoveSkill,
    RemoveSkillRepo, RenameSession, SendInput, SessionContextBreakdown, SessionSummary,
    SetCommunication, SetCompactStyle, SetEffort, SetEscalateModel, SetMaxSalvages,
    SetMaxToolRounds, SetModel, SetPeerScope, SetStreamIdle, SetSubagentModel, SetSubagentTimeout,
    SetTitleModel, SetVisionModel, ShowPrompt, SkillBody, SkillMeta, Steer, SyncSkills, Undo,
    UseSkill, Withdraw,
};
use agent::{
    AdvisorDefaults, AdvisorStatus, AdvisorStatusOf, SetAdvisor, SetAdvisorDefaults, SyncBacklog,
};
use agent::{
    Authorize, Authorized, CloseResult, CreateSession, CreateSessionOptions, LeaveSession,
    ListProjectStyles, PeerDeliver, PeerEnvelope, PrivateTrees, ProjectStyle, ResumePrivate,
    SessionCost, SessionCostQuery, SessionOpen, SetProjectStyle, SetSessionProject, SpendReport,
    SpendReportQuery,
};
use agent::{AwsProfiles, ListAwsProfiles, SetAwsProfile};
use agent::{EndlessConfig, MemoryMode, SessionMemory, ViewLine};
use agent::{GetFavoriteModels, SetModelFavorite};
use app::{
    AppDoc, AppToolResult, Apps, CallArgs, CallStatusOf, CallTool, CloseApp, GetApp, PublishFromUi,
};
use artifact::search::ArtifactMatchContext;
use base64::Engine as _;
use browser::{BROWSER_ACTOR, Browsers, Screenshot};
use codeindex::{
    CODEINDEX_ACTOR, ChangeLens, CodeindexActor, FitLens, HereLens, IndexStats, LensMeta,
    LocateAnchor, Located,
};
use computer::{COMPUTER_ACTOR, Computer, Permissions, ScreenContext};
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
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sessionsync::types::{SessionOrphan, SessionOwner, SessionsStatus};
use signal::{Allow, Link, RemoveAllowed, SIGNAL_ACTOR, Signal, SignalStatus, Status, Unlink};
use slack::{SLACK_ACTOR, Slack, SlackStatus};
use telegram::{PairingLink, TelegramStatus};
use tokio::sync::broadcast;
use tokio::time::Instant;
use tools::git::SyncResult;
use tools::{
    AddRepo, AddServer, Answer, Approval, ApprovalId, ApprovalRequested, ApprovalsState, AskId,
    AskRequested, CancelJob, CancelOrigin, Decision, Gate, GetApprovals, GetPtyIdle, JOBS_ACTOR,
    JobId, JobView, Jobs, ListJobs, ListServers, ListTools, Mcp, McpLogin, McpLogout, McpProgress,
    McpServer, Progress, RemoveServer, RepoInfo, Resolve, SetApprovals, SetGate, SetHidden,
    SetPtyIdle, SyncMcp, ToolRegistry, ToolSpec,
};
use tools::{
    Key, ListTerminals, PtyAttach, PtyChunk, PtyInput, PtyOut, PtyResize, PtyScreen,
    PtyScreenReply, PtyViewport, Retention, TerminalSummary,
};
use tools::{LiveCounts, LiveCountsRow};
use uuid::Uuid;
use voice::{SpeechId, VoiceStatus};

use crate::bridge::Event;
use crate::forward;
use crate::pathac::PathCompletion;
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
    /// `agent.model`: the model new sessions start on (live, 82 §3).
    pub default_model: String,
    /// `agent.effort`: the reasoning effort new sessions start on (live, 83 §3);
    /// `None` = the provider default. Absent from an older harness → `None`.
    #[serde(default)]
    pub default_effort: Option<String>,
    /// `agent.favorite_models` (127 §1): starred model ids in starring order,
    /// shown first in every picker. Absent from an older harness → empty.
    #[serde(default)]
    pub favorite_models: Vec<String>,
    /// The global approval switch (`tools.approvals`).
    pub approvals: ApprovalsState,
    /// `agent.compact_style`.
    pub compact_style: CompactStyle,
    /// `agent.subagents.max_runtime_secs`.
    pub subagent_timeout_secs: u64,
    /// `schedule.run_timeout_secs` (88): seconds a routine run may take before it is
    /// cancelled and its history row says `timeout`; a routine's own `run_timeout:`
    /// overrides it. `0` from an older harness.
    #[serde(default)]
    pub schedule_run_timeout_secs: u64,
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
    /// 125 §6.5: the advisor's remembered pick and tuning keys (`advisor:` section).
    /// Absent from an older harness → defaults.
    #[serde(default)]
    pub advisor: AdvisorDefaults,
}

/// The wire API. `#[rpc(server, client)]` generates `ApiServer` (+
/// `into_rpc()`) and `ApiClient`, the same methods on any jsonrpsee client —
/// the in-process [`crate::client::Local`] or a WebSocket (63 §2.3).
#[rpc(server, client)]
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
    /// `files` are image files (17 §2) attached to the message. `delivery`
    /// (default `steer`) says how it lands on a busy session (62 §0.3). `id`
    /// is the client's idempotency key: a retry with the same id routes once.
    /// Returns once queued.
    #[method(name = "send_input", with_extensions)]
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
        delivery: Option<Delivery>,
        id: Option<InputId>,
    ) -> RpcResult<SessionId>;
    /// PATH-AC: candidates for the path token under `cursor` in `input`,
    /// listed by the engine running `session` (`None`: this engine, its
    /// cwd). Relative paths resolve against the session's cwd; `~`/`$VAR`
    /// expand for the lookup only. Empty when nothing triggers, the
    /// directory is missing or the owner is unreachable: never an error.
    #[method(name = "path_complete", with_extensions)]
    async fn path_complete(
        &self,
        session: Option<SessionId>,
        input: String,
        cursor: usize,
    ) -> RpcResult<PathCompletion>;
    /// Queue a user message after the current tool batch (or, with
    /// `delivery: next_turn`, after the turn); returns the id (minted when
    /// omitted). `files` are image files (17 §2).
    #[method(name = "steer", with_extensions)]
    async fn steer(
        &self,
        session: SessionId,
        text: String,
        id: Option<InputId>,
        files: Vec<PathBuf>,
        delivery: Option<Delivery>,
    ) -> RpcResult<InputId>;
    /// A peer message for `session` forwarded by the engine of its sender
    /// (69 P): the owner's intake. Mesh-internal — accepted only over
    /// `theuth/rpc/1` (the `Forwarded` marker), never from a client; the
    /// supervisor refuses when the session is not owned here (no take).
    /// Returns the owner's receipt (`accepted`); a refusal is the error the
    /// sender records as a `failed` receipt.
    #[method(name = "peer_deliver", with_extensions)]
    async fn peer_deliver(
        &self,
        session: SessionId,
        envelope: PeerEnvelope,
    ) -> RpcResult<PeerReceipt>;
    /// One message to many sessions (62 §0.1): the selector is resolved on
    /// the harness, each row is authorized like `send_input`, and the reply
    /// carries one outcome per session. `dry_run: true` previews the rows.
    #[method(name = "broadcast", with_extensions)]
    async fn broadcast(&self, req: BroadcastRequest) -> RpcResult<BroadcastReceipt>;
    /// Every session with a live actor or a running job plus every root, with
    /// [`AgentInfo::activity`]; the broadcast picker's source (62 §0.6).
    #[method(name = "list_activity", with_extensions)]
    async fn list_activity(&self) -> RpcResult<Vec<AgentInfo>>;
    /// Pull a queued message back out; returns its text.
    #[method(name = "withdraw", with_extensions)]
    async fn withdraw(&self, session: SessionId, id: InputId) -> RpcResult<String>;
    /// Messages waiting for the current tool batch to end.
    #[method(name = "list_queued", with_extensions)]
    async fn list_queued(&self, session: SessionId) -> RpcResult<Vec<QueuedInput>>;
    /// Abort the in-flight turn; `scope` `tree` also stops every running descendant (133 §4; default `root`).
    #[method(name = "cancel", with_extensions)]
    async fn cancel(&self, session: SessionId, scope: Option<CancelScope>) -> RpcResult<()>;
    /// Background jobs of a session (14 §2): id, state, seconds, log path, tail.
    #[method(name = "list_jobs", with_extensions)]
    async fn list_jobs(&self, session: SessionId) -> RpcResult<Vec<JobView>>;
    /// Every session with a running job, a live PTY or a pending reminder
    /// (ids only): the desktop's sidebar snapshot after connect / resync.
    #[method(name = "list_live_counts")]
    async fn list_live_counts(&self) -> RpcResult<Vec<LiveCountsRow>>;
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
    async fn pty_resize(
        &self,
        session: SessionId,
        job: JobId,
        rows: u16,
        cols: u16,
    ) -> RpcResult<()>;
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
    /// Capture the displays a pointer path visited, draw the path, read the
    /// accessibility tree under it, write the images under
    /// `<data_dir>/attachments/`, answer the `[screen …]` lines (design doc
    /// 96 §6.3). Local peers only; "disabled" when `computer.enabled` is
    /// false; a Screen Recording denial names the process.
    #[method(name = "screen_context", with_extensions)]
    async fn screen_context(
        &self,
        req: computer::ScreenContextReq,
    ) -> RpcResult<computer::ScreenContextReply>;
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
    /// Where the session's frame tokens go (70 COMPACT-SIZE): head images and
    /// text, tail messages/tokens, system, the calibrated total estimate the
    /// gauge shows after a compaction, and the provider's last billed context.
    /// Live figures from an idle actor; the stored frame otherwise.
    #[method(name = "session_context_breakdown", with_extensions)]
    async fn session_context_breakdown(&self, session: SessionId) -> RpcResult<ContextBreakdown>;
    /// 137 §11.2: the session's memory mode, its view, settle state,
    /// compactor stats and backfill progress; `mode: epochs` + an empty view
    /// for an epochs session.
    #[method(name = "session_memory", with_extensions)]
    async fn session_memory(&self, session: SessionId) -> RpcResult<SessionMemory>;
    /// 137 §9 `zoom`: the two children of line `id+n` (`n == 1`: the whole message).
    #[method(name = "endless_zoom", with_extensions)]
    async fn endless_zoom(&self, session: SessionId, id: u64, n: u64) -> RpcResult<Vec<ViewLine>>;
    /// 137 §9 `recall`: FTS over the level-0 lines (raw fallback under 2 hits); `limit` default 8.
    #[method(name = "endless_recall", with_extensions)]
    async fn endless_recall(
        &self,
        session: SessionId,
        query: String,
        limit: Option<u8>,
    ) -> RpcResult<Vec<ViewLine>>;
    /// 137 8.6 `/endless on|off` over RPC; idle sessions only (error `busy`).
    #[method(name = "set_session_memory", with_extensions)]
    async fn set_session_memory(
        &self,
        session: SessionId,
        mode: MemoryMode,
    ) -> RpcResult<SessionMemory>;
    /// 137 R1-D2b: the session's view budget in bytes (16_000..=256_000);
    /// idle or running — it takes effect at the next turn boundary.
    #[method(name = "set_session_view_bytes", with_extensions)]
    async fn set_session_view_bytes(
        &self,
        session: SessionId,
        bytes: u32,
    ) -> RpcResult<SessionMemory>;
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
    /// Persist `schedule.run_timeout_secs` (88); runs fired from then on use it.
    #[method(name = "set_schedule_run_timeout")]
    async fn set_schedule_run_timeout(&self, seconds: u64) -> RpcResult<()>;
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
    /// Model-call spend of the durable ledger (128 Rev 3): the last `days`
    /// local calendar days (clamped to 1..=31, oldest first), the month to
    /// date, and the window's top owners by root session. Re-read on
    /// `ModelUsageChanged`.
    #[method(name = "spend_report")]
    async fn spend_report(&self, days: u32) -> RpcResult<SpendReport>;
    /// Set a session's title by hand; `TitleChanged` follows on the stream.
    #[method(name = "rename_session", with_extensions)]
    async fn rename_session(&self, session: SessionId, title: String) -> RpcResult<()>;
    /// Pin or unpin a session; pinned sessions lead `list_sessions`.
    #[method(name = "pin_session", with_extensions)]
    async fn pin_session(&self, session: SessionId, pinned: bool) -> RpcResult<()>;
    /// Archive or unarchive a root; archived trees leave the sidebar but stay
    /// in `list_sessions` (flagged) and search (51 §3).
    #[method(name = "archive_session", with_extensions)]
    async fn archive_session(&self, session: SessionId, archived: bool) -> RpcResult<()>;
    /// Delete a session and its subagents for good; a `SessionDeleted` per removed session
    /// follows on the stream. Refused while any of them is mid-turn.
    #[method(name = "delete_session", with_extensions)]
    async fn delete_session(&self, session: SessionId) -> RpcResult<()>;
    /// Take `session` here (68 §4.2): `reason` is `user` (continue here, at
    /// once), `orphan` (a resumed checkout) or `write` (owner unreachable ≥
    /// `sessions.takeover_secs`). Returns the new owner/epoch;
    /// `SessionOwnerChanged` follows on the stream.
    #[method(name = "session_take", with_extensions)]
    async fn session_take(&self, session: SessionId, reason: String) -> RpcResult<SessionOwner>;
    /// Who owns `session` and whether that is this engine (68 §5).
    #[method(name = "session_owner", with_extensions)]
    async fn session_owner(&self, session: SessionId) -> RpcResult<SessionOwner>;
    /// Turns this engine wrote as a superseded owner and never delivered
    /// (68 §4.3), one entry per epoch; empty when none.
    #[method(name = "session_orphans", with_extensions)]
    async fn session_orphans(&self, session: SessionId) -> RpcResult<Vec<SessionOrphan>>;
    /// This engine's replication summary (68 §10): id, sessions owned here
    /// / elsewhere, log head, last snapshot.
    #[method(name = "sessions_status")]
    async fn sessions_status(&self) -> RpcResult<SessionsStatus>;
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
    /// Every styled project's icon and colour (51 §3 as-built); projects
    /// without a row use the client's neutral default.
    #[method(name = "list_project_styles")]
    async fn list_project_styles(&self) -> RpcResult<Vec<ProjectStyle>>;
    /// Set project `name`'s icon (lucide name) and colour (palette key), both
    /// allow-listed; both `None` clears the style. No event, like
    /// `pin_session`: clients refetch `list_project_styles`.
    #[method(name = "set_project_style")]
    async fn set_project_style(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
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
    #[method(name = "resolve_approval", with_extensions)]
    async fn resolve_approval(&self, id: ApprovalId, decision: Decision) -> RpcResult<bool>;
    /// Answer a pending `ask_user`: one answer per question, or `None` to
    /// dismiss the set, which also cancels the turn. `true` if it was still pending.
    #[method(name = "answer_ask", with_extensions)]
    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> RpcResult<bool>;
    /// A client's answer to a `PagePreviewRequested` (120 §11.2): `Started`
    /// extends the wait, the first `Done`/`Failed` resolves it. `true` if the
    /// preview was still pending.
    #[method(name = "answer_page_preview")]
    async fn answer_page_preview(
        &self,
        id: artifact::preview::PreviewId,
        answer: artifact::preview::PagePreviewAnswer,
    ) -> RpcResult<bool>;
    /// Every catalog model whose provider is configured.
    #[method(name = "list_models")]
    async fn list_models(&self) -> RpcResult<Vec<ModelInfo>>;
    /// AWS SDK-discovered profiles on this harness, plus its inference selection.
    #[method(name = "list_aws_profiles")]
    async fn list_aws_profiles(&self) -> RpcResult<AwsProfiles>;
    /// Select the harness-wide Bedrock inference profile; `None` restores the AWS default chain.
    #[method(name = "set_aws_profile")]
    async fn set_aws_profile(&self, profile: Option<String>) -> RpcResult<AwsProfiles>;
    /// Switch `session` to `model`. `manual` (default `true`): a person picked it in a model
    /// picker, so it also becomes the default for new sessions (82 §2) and `DefaultModelChanged`
    /// follows on the stream; `false` switches the session only.
    #[method(name = "set_model", with_extensions)]
    async fn set_model(
        &self,
        session: Option<SessionId>,
        model: String,
        manual: Option<bool>,
    ) -> RpcResult<ModelInfo>;
    /// Star (`favorite: true`) or unstar `model` for every device on this engine (127 §2).
    /// `model` resolves like `set_model` (unknown ids fail the same way). Returns the new
    /// list in starring order; `FavoriteModelsChanged` follows on the stream when it changed.
    #[method(name = "set_model_favorite")]
    async fn set_model_favorite(&self, model: String, favorite: bool) -> RpcResult<Vec<String>>;
    /// Set (or clear with `None`) the reasoning effort for `session` from its next turn. Values
    /// come from the model's catalog entry (`ModelInfo.efforts`, or an integer token budget within
    /// `budget_min..budget_max`); `EffortChanged` follows on the stream. `manual` (default `true`):
    /// a person picked it in an effort picker, so it also becomes the default for new sessions
    /// (`agent.effort`, 83 §2) and `DefaultEffortChanged` follows; `false` changes the session only.
    #[method(name = "set_effort", with_extensions)]
    async fn set_effort(
        &self,
        session: Option<SessionId>,
        effort: Option<String>,
        manual: Option<bool>,
    ) -> RpcResult<()>;
    /// 125 §6.3: turn the advisor of `session` on with `model` (`provider/model`; creates the
    /// hidden advisor child or re-picks its model) or off with `None`. The pick is remembered as
    /// `advisor.model` / `advisor.effort`; `AdvisorChanged` follows on the stream.
    #[method(name = "set_advisor", with_extensions)]
    async fn set_advisor(
        &self,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
    ) -> RpcResult<AdvisorStatus>;
    /// 125 §6.3: the advisor status of `session` (`state: off` when it has none).
    #[method(name = "advisor_status", with_extensions)]
    async fn advisor_status(&self, session: SessionId) -> RpcResult<AdvisorStatus>;
    /// Persist `advisor.effort` (`None` clears); the remembered pick's effort (125 §6.5).
    #[method(name = "set_advisor_default_effort")]
    async fn set_advisor_default_effort(
        &self,
        effort: Option<String>,
    ) -> RpcResult<AdvisorDefaults>;
    /// Persist `advisor.max_notes_per_update` (1–32).
    #[method(name = "set_advisor_notes_per_update")]
    async fn set_advisor_notes_per_update(&self, n: u8) -> RpcResult<AdvisorDefaults>;
    /// Persist `advisor.immune_turns` (0–10; 0 disables the window).
    #[method(name = "set_advisor_immune_turns")]
    async fn set_advisor_immune_turns(&self, n: u8) -> RpcResult<AdvisorDefaults>;
    /// Persist `advisor.sync_backlog` (`off` | `1` | `3` | `5`).
    #[method(name = "set_advisor_sync_backlog")]
    async fn set_advisor_sync_backlog(&self, v: SyncBacklog) -> RpcResult<AdvisorDefaults>;
    /// An ACP child's harness config options (75 §8): model, effort/thinking,
    /// mode … as the harness lists them, with current values. Errors when the
    /// session is not an external harness or its process is gone.
    #[method(name = "harness_options", with_extensions)]
    async fn harness_options(&self, session: SessionId) -> RpcResult<Vec<acp::ConfigOption>>;
    /// Set one harness config option on an ACP child (75 §8): `config_id` is
    /// the option's id or the alias `model` / `effort`; `value` one of its
    /// `options[].value`. Mid-turn the change is queued for the turn's end
    /// (`pending: true`). A missing option or an unlisted value is refused
    /// with the list. The model change follows as `HarnessModelChanged`.
    #[method(name = "set_harness_option", with_extensions)]
    async fn set_harness_option(
        &self,
        session: SessionId,
        config_id: String,
        value: String,
    ) -> RpcResult<acp::SetOptionResult>;
    /// Resume a stopped ACP child in its prior harness context (75 §10): a
    /// new harness process, `session/load` on the stored ACP session id, no
    /// prompt; a still-live process for the session is killed first. Errors
    /// when the session is native, belongs to a remote node, or has no
    /// stored harness session id.
    #[method(name = "resume_child", with_extensions)]
    async fn resume_child(&self, session: SessionId) -> RpcResult<()>;
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
    /// A peer message from `session` to `to` (`parent`, `session:<uuid>`,
    /// `peer:<name>`, a child's name), routed like the model's `send_message`
    /// tool without a wait; the MCP back-channel's `send_message` (75 §9).
    /// `Err` is a refused admission and nothing was sent.
    #[method(name = "send_message", with_extensions)]
    async fn send_message(
        &self,
        session: SessionId,
        to: String,
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
    /// Live documents, newest first (design doc 59 §6.1); `limit` defaults to 100.
    #[method(name = "doc_list")]
    async fn doc_list(
        &self,
        kind: Option<document::DocKind>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<document::DocSummary>>;
    /// The document bound to `binding`. A knowledge binding whose concept
    /// exists but has no document yet gets one created from its file (59
    /// §7.1, on demand), an agent-charter binding from the roster file's body;
    /// `None` only when there is nothing to bind to.
    #[method(name = "doc_resolve", with_extensions)]
    async fn doc_resolve(&self, binding: document::Binding) -> RpcResult<Option<document::DocId>>;
    /// The document bound to an agent-charter `binding`, created from `markdown` when there is none
    /// (the Hire wizard's draft, before `agents/<name>.md` exists). Other bindings: `owner_creates`.
    #[method(name = "doc_ensure", with_extensions)]
    async fn doc_ensure(
        &self,
        binding: document::Binding,
        markdown: String,
    ) -> RpcResult<document::DocId>;
    /// Lossy markdown of the whole document (what its export writes) — Hire copies it into the charter body.
    #[method(name = "doc_markdown")]
    async fn doc_markdown(&self, doc: document::DocId) -> RpcResult<String>;
    /// Export `doc` (72): `markdown` = `doc_markdown`'s text, `pdf` = the print lowering through Typst
    /// with `assets` (client-rendered diagram SVGs by block id) as figures. `dest` (owner only, under
    /// the home directory) is written owner-only and atomically; `None` stores an artifact instead.
    #[method(name = "doc_export", with_extensions)]
    async fn doc_export(
        &self,
        doc: document::DocId,
        format: artifact::ExportFormat,
        dest: Option<PathBuf>,
        assets: Vec<artifact::ExportAsset>,
    ) -> RpcResult<artifact::ExportResult>;
    /// Open `doc`: its metadata, the Yjs state missing from `sv` (base64 v1
    /// state vector; absent = everything) as ≤ 48 KiB base64 chunks, and the
    /// current awareness states.
    #[method(name = "doc_open", with_extensions)]
    async fn doc_open(
        &self,
        doc: document::DocId,
        sv: Option<String>,
    ) -> RpcResult<document::DocOpen>;
    /// One base64 v1 update from this connection; refused with `UpdateTooLarge` over 48 KiB and `DocTooLarge` past the document cap.
    #[method(name = "doc_update", with_extensions)]
    async fn doc_update(&self, doc: document::DocId, update: String) -> RpcResult<()>;
    /// One base64 awareness update (y-protocols) from this connection; relayed, never stored.
    #[method(name = "doc_awareness", with_extensions)]
    async fn doc_awareness(&self, doc: document::DocId, state: String) -> RpcResult<()>;
    /// Every `DocEvent` of `doc` not caused by this connection: other
    /// clients' updates and awareness, exports, snapshots, close.
    #[subscription(name = "doc_subscribe", unsubscribe = "doc_unsubscribe", item = document::DocEvent, with_extensions)]
    async fn doc_subscribe(&self, doc: document::DocId) -> SubscriptionResult;
    /// Kept snapshots of `doc`, newest first (59 §4.6).
    #[method(name = "doc_snapshots")]
    async fn doc_snapshots(&self, doc: document::DocId) -> RpcResult<Vec<document::SnapshotInfo>>;
    /// Import a snapshot's markdown as one diff transaction (59 §4.6: history is kept, not rewound).
    #[method(name = "doc_restore", with_extensions)]
    async fn doc_restore(&self, doc: document::DocId, snapshot: String) -> RpcResult<()>;
    /// Revert the last agent edit of `doc` as the owner (59 §5.5 page menu); one level, in memory.
    #[method(name = "doc_undo_agent", with_extensions)]
    async fn doc_undo_agent(&self, doc: document::DocId) -> RpcResult<()>;
    /// A new note (59 §5.3, §7.3): `kind: note`, derived `documents/<id>/note.md`, headed `# <title>` unless `markdown` has one.
    #[method(name = "doc_create", with_extensions)]
    async fn doc_create(
        &self,
        title: String,
        markdown: Option<String>,
    ) -> RpcResult<document::DocSummary>;
    /// Retitle a note: meta, index and its first block when that is a level-1 heading. Other kinds: `title_owned_by_binding`.
    #[method(name = "doc_rename", with_extensions)]
    async fn doc_rename(&self, doc: document::DocId, title: String) -> RpcResult<()>;
    /// Move a note to `documents/.trash/<id>` (swept after 30 days); subscribers get `Closed{deleted}`. Other kinds: `delete_via_owner`.
    #[method(name = "doc_delete", with_extensions)]
    async fn doc_delete(&self, doc: document::DocId) -> RpcResult<()>;
    /// Whether the documents helper (`theuth-documents-<version>` beside `t`, 59 §3.2) is there and how its
    /// process is doing (`process`: `running` | `crashed{stderr}` | `missing` | `starting`); a missing or
    /// crashed helper disables notes, knowledge editing and meeting notes. Answers whether documents run or not.
    #[method(name = "documents_status")]
    async fn documents_status(&self) -> RpcResult<document::HelperStatus>;
    /// How this engine handled peers' document snapshots since start (69 E2): `merged`, `unchanged`,
    /// `resnapshots`, `noop_applies_avoided`. Served on any engine; never persisted.
    #[method(name = "doc_stats")]
    async fn doc_stats(&self) -> RpcResult<document::DocStats>;
    /// A document's members and share links (59 §8; owner only).
    #[method(name = "doc_members", with_extensions)]
    async fn doc_members(&self, doc: document::DocId) -> RpcResult<document::Members>;
    /// Rename or re-role a member (59 §8.1; owner only). Demoting an editor closes their
    /// subscriptions with `Closed{demoted}`; their editor re-opens read-only.
    #[method(name = "doc_member_set", with_extensions)]
    async fn doc_member_set(
        &self,
        doc: document::DocId,
        member: String,
        role: Option<document::Role>,
        name: Option<String>,
    ) -> RpcResult<()>;
    /// Remove a member (59 §8.4; owner only): `Closed{revoked}`, their connections dropped, the next
    /// handshake from that static finds no membership.
    #[method(name = "doc_member_revoke", with_extensions)]
    async fn doc_member_revoke(&self, doc: document::DocId, member: String) -> RpcResult<()>;
    /// A new share link (59 §8.2; owner only): `https://<relay>/d#1.<mesh device id | engine id>.<doc>.<secret>` (69),
    /// returned once — the harness keeps only `sha256(secret)`. Needs Link enabled (the relay host).
    #[method(name = "doc_link_create", with_extensions)]
    async fn doc_link_create(
        &self,
        doc: document::DocId,
        role: document::Role,
        name: Option<String>,
        expires_ms: Option<i64>,
    ) -> RpcResult<document::LinkCreated>;
    /// Stop new joins through `link` (59 §8.4; owner only); its members stay until revoked.
    #[method(name = "doc_link_revoke", with_extensions)]
    async fn doc_link_revoke(&self, doc: document::DocId, link: String) -> RpcResult<()>;
    /// A guest connection (31 §4.3 mode 2) proves a link secret for this connection and becomes a
    /// member (59 §8.3): `proof` = base64url `HMAC-SHA256(sha256(secret), handshake_hash ‖ guest_pub)`;
    /// the link is the one the proof verifies against (the URL carries no link id).
    /// Errors: `link_unknown`, `link_revoked`, `link_expired`, `bad_proof`, `members_full`, `sharing_off`.
    #[method(name = "doc_join", with_extensions)]
    async fn doc_join(
        &self,
        doc: document::DocId,
        proof: String,
        name: Option<String>,
    ) -> RpcResult<document::Joined>;
    /// A member renames themself (59 §8.5; editors only — `viewer_readonly` otherwise).
    #[method(name = "doc_set_name", with_extensions)]
    async fn doc_set_name(&self, doc: document::DocId, name: String) -> RpcResult<()>;
    /// Notebooks (144 §13.3): live ones newest first; `archived` includes archived. Any engine, from the replica.
    #[method(name = "notebook_list")]
    async fn notebook_list(&self, archived: Option<bool>) -> RpcResult<Vec<document::Notebook>>;
    /// One notebook with counts and its live items. Any engine; mode-2 guests see the notebooks they are members of.
    #[method(name = "notebook_get", with_extensions)]
    async fn notebook_get(&self, id: document::DocId) -> RpcResult<document::NotebookDetail>;
    /// A new notebook (144 §3.2): a `DocKind::Notebook` document named `title` whose body is `description`,
    /// plus its `notebooks` row. Hub only (`-32011` → forwarded).
    #[method(name = "notebook_create", with_extensions)]
    async fn notebook_create(
        &self,
        title: String,
        description: Option<String>,
        project: Option<String>,
    ) -> RpcResult<document::Notebook>;
    /// Rename, re-project (`project: ""` clears) or (un)archive a notebook. Hub only.
    #[method(name = "notebook_update", with_extensions)]
    async fn notebook_update(
        &self,
        id: document::DocId,
        title: Option<String>,
        project: Option<String>,
        archived: Option<bool>,
    ) -> RpcResult<()>;
    /// What `notebook_delete` would do (144 §3.4): note count, exclusive sources, sources also in other notebooks.
    #[method(name = "notebook_delete_preview")]
    async fn notebook_delete_preview(
        &self,
        id: document::DocId,
    ) -> RpcResult<document::DeletePreview>;
    /// Trash the notebook document, soft-remove its items and, when `delete_exclusive_sources`, the sources no
    /// other live notebook holds. Notes are never deleted; members get `Closed{deleted}`. Hub only.
    #[method(name = "notebook_delete", with_extensions)]
    async fn notebook_delete(
        &self,
        id: document::DocId,
        delete_exclusive_sources: bool,
    ) -> RpcResult<()>;
    /// Live items of a notebook, oldest first. Any engine; membership-checked for guests.
    #[method(name = "notebook_items_list", with_extensions)]
    async fn notebook_items_list(
        &self,
        notebook: document::DocId,
    ) -> RpcResult<Vec<document::NotebookItem>>;
    /// Add (or revive) one item row. Hub only.
    #[method(name = "notebook_item_add", with_extensions)]
    async fn notebook_item_add(
        &self,
        notebook: document::DocId,
        kind: document::ItemKind,
        item: String,
    ) -> RpcResult<document::NotebookItem>;
    /// Soft-remove one item row; answers how many other live notebooks still hold the item. Hub only.
    #[method(name = "notebook_item_remove", with_extensions)]
    async fn notebook_item_remove(
        &self,
        notebook: document::DocId,
        kind: document::ItemKind,
        item: String,
    ) -> RpcResult<document::ItemRemoved>;
    /// Sessions bound to `notebook`, newest first (144 §8). Owner only — never on the guest allow-list (D13).
    #[method(name = "notebook_chats", with_extensions)]
    async fn notebook_chats(&self, notebook: document::DocId) -> RpcResult<Vec<SessionSummary>>;
    /// Bind `session` to a notebook (`sessions.notebook`), or unbind with `None`. Owner only.
    #[method(name = "session_set_notebook", with_extensions)]
    async fn session_set_notebook(
        &self,
        session: SessionId,
        notebook: Option<document::DocId>,
    ) -> RpcResult<()>;
    /// Add a source to a notebook (144 §4): pre-flight, the `new` row and card at once, then the ingest job
    /// (`SourceEvent` on `subscribe_events`). Exactly one origin field. Hub only.
    #[method(name = "source_add", with_extensions)]
    async fn source_add(
        &self,
        args: document::ingest::SourceAddArgs,
    ) -> RpcResult<document::Source>;
    /// One source's card. Any engine; membership-checked for guests.
    #[method(name = "source_get", with_extensions)]
    async fn source_get(&self, id: document::DocId) -> RpcResult<document::Source>;
    /// Sources of `notebook`, or every source (unfiled included). Any engine.
    #[method(name = "source_list")]
    async fn source_list(
        &self,
        notebook: Option<document::DocId>,
    ) -> RpcResult<Vec<document::Source>>;
    /// Re-run the ingest from the origin or the blob (144 §4.2); refused while queued or running. Hub only.
    #[method(name = "source_refresh", with_extensions)]
    async fn source_refresh(&self, id: document::DocId) -> RpcResult<document::Source>;
    /// Resolve `theuth://` hrefs (144 §6.4) to citations with their drift status, positionally. Any engine;
    /// `available: false` for trashed targets or ones the caller may not read.
    #[method(name = "cite_resolve", with_extensions)]
    async fn cite_resolve(&self, hrefs: Vec<String>) -> RpcResult<Vec<document::Citation>>;
    /// Hybrid block search over notes, sources, insights and notebook descriptions (144 §7.4). Any engine.
    #[method(name = "nb_search")]
    async fn nb_search(
        &self,
        args: document::index::NbSearchArgs,
    ) -> RpcResult<Vec<document::index::BlockHit>>;
    /// Save the assistant message at `turn` of `session` as a new note in `notebook`, chips intact (144 §8). Owner only.
    #[method(name = "save_answer_as_note", with_extensions)]
    async fn save_answer_as_note(
        &self,
        session: SessionId,
        turn: u32,
        notebook: document::DocId,
    ) -> RpcResult<agent::notebooks::SavedAnswer>;
    /// Append the assistant message at `turn` of `session` to `note` under `## From chat · <date>` (144 §8). Owner only.
    #[method(name = "promote_answer", with_extensions)]
    async fn promote_answer(
        &self,
        session: SessionId,
        turn: u32,
        note: document::DocId,
    ) -> RpcResult<()>;
    /// Row counts and freshness of the code index for `root` (default: the
    /// git top-level of the process cwd; design doc 60 §10). Opens the
    /// index without reconciling it.
    #[method(name = "codeindex_stats")]
    async fn codeindex_stats(&self, root: Option<String>) -> RpcResult<IndexStats>;
    /// Verify board anchors against the working tree under `root` (71 §2.5,
    /// M11): one `Located` per anchor, in order — `same` / `changed` /
    /// `moved` (new range) / `missing`. Read-only; `root` must be a git
    /// top-level and every `path` must stay under it (the whole batch errors
    /// otherwise).
    #[method(name = "code_locate")]
    async fn code_locate(
        &self,
        root: String,
        anchors: Vec<LocateAnchor>,
    ) -> RpcResult<Vec<Located>>;
    /// CodeMap Here lens (73 §5.2): the stored ranked map of an already
    /// indexed `root` cut to `limit` rows (default 2 000) with badges;
    /// `focus`/`edited` personalise the rank. Store reads only, never a
    /// walk; `ASK_FAILED` when the root was never indexed.
    #[method(name = "codemap_here")]
    async fn codemap_here(
        &self,
        root: String,
        focus: Option<Vec<String>>,
        edited: Option<Vec<String>>,
        limit: Option<u32>,
    ) -> RpcResult<HereLens>;
    /// CodeMap Fit lens (73 §5.2): the butterfly around `symbol` (a moniker,
    /// a name, `path:line` or a symbol id) — callers, focus, callees — cut to
    /// `limit` nodes (default 20) with counted remainders.
    #[method(name = "codemap_fit")]
    async fn codemap_fit(
        &self,
        root: String,
        symbol: String,
        limit: Option<u32>,
    ) -> RpcResult<FitLens>;
    /// CodeMap Change lens (73 §5.2): `code_impact` over the open index in
    /// four bands (changed, direct, transitive, tests); neither `git_range`
    /// nor `files` means `WORKTREE`.
    #[method(name = "codemap_change")]
    async fn codemap_change(
        &self,
        root: String,
        git_range: Option<String>,
        files: Option<Vec<String>>,
        include_tests: Option<bool>,
    ) -> RpcResult<ChangeLens>;
    /// CodeMap touch (73 §5.2): stat and re-parse only `paths` (root-relative;
    /// a path outside the root fails the call), or force a full reconcile when
    /// `paths` is null. Publishes `CodeReconciled`.
    #[method(name = "codemap_touch")]
    async fn codemap_touch(&self, root: String, paths: Option<Vec<String>>) -> RpcResult<LensMeta>;
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
    /// Telegram bot identity, poller state, trust lists, groups and bound threads (98 §13).
    #[method(name = "telegram_status")]
    async fn telegram_status(&self) -> RpcResult<TelegramStatus>;
    /// Pair the owner (98 §10.4): with `owner`, set `telegram.owner` live and in the
    /// config file and answer `None`; without, mint a single-use code and answer the
    /// `t.me/<bot>?start=<code>` deep link (needs the daemon).
    #[method(name = "telegram_link")]
    async fn telegram_link(&self, owner: Option<u64>) -> RpcResult<Option<PairingLink>>;
    /// Add a user id to `telegram.allowed` (live and in the config file).
    #[method(name = "telegram_allow")]
    async fn telegram_allow(&self, user: u64) -> RpcResult<()>;
    /// Remove a user id from `telegram.allowed`.
    #[method(name = "telegram_remove_allowed")]
    async fn telegram_remove_allowed(&self, user: u64) -> RpcResult<()>;
    /// Add a chat id (`-100…`) to `telegram.groups` (live and in the config file).
    #[method(name = "telegram_allow_group")]
    async fn telegram_allow_group(&self, chat: i64) -> RpcResult<()>;
    /// Remove a chat id from `telegram.groups`.
    #[method(name = "telegram_remove_group")]
    async fn telegram_remove_group(&self, chat: i64) -> RpcResult<()>;
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
    /// Stop the whole process. Logged with the calling connection (79 smoke bug 7).
    #[method(name = "shutdown", with_extensions)]
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
    /// Every row of the replicated device registry, revoked included (69 §3).
    #[method(name = "devices_list")]
    async fn devices_list(&self) -> RpcResult<Vec<link::DeviceInfo>>;
    /// Revoke a device (69 §3): `revoked_ms` set, conns closed; replicates to every hub.
    #[method(name = "device_revoke")]
    async fn device_revoke(&self, id: String) -> RpcResult<()>;
    /// Rename a device in the replicated registry (69 §3).
    #[method(name = "device_rename")]
    async fn device_rename(&self, id: String, name: String) -> RpcResult<()>;
    /// Start a pairing against the MESH identity (69 decision 4): the QR
    /// string (4 parts, pending id last), its expiry and the pending id. Any
    /// engine may call it; the hub accepts the device. `name` is what the QR
    /// shows; `None` → the mesh's name.
    #[method(name = "device_pair_start")]
    async fn device_pair_start(&self, name: Option<String>) -> RpcResult<link::PairingPayload>;
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
    /// Every installed sensor with its `# arg:` header, description and
    /// install state (108 §9.2) for the Routines form and the `routines` skill.
    #[method(name = "list_sensors")]
    async fn list_sensors(&self) -> RpcResult<Vec<schedule::SensorInfo>>;
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
    /// Start a live spoken conversation with `session` (76 §5.1); events arrive
    /// as `TalkEvent` on `subscribe_events`. Local peers pass `sink: "device"`
    /// and `sample_rate: null` (the daemon's speaker and mic); a relay peer must
    /// pass `sink: "client"` and the rate of the mono i16 PCM it will push with
    /// `talk_audio` — refused otherwise. One talk per daemon: a second start
    /// answers `already talking`; `talk is disabled` when the plugin is off.
    #[method(name = "talk_start", with_extensions)]
    async fn talk_start(
        &self,
        session: SessionId,
        sink: talk::Sink,
        sample_rate: Option<u32>,
    ) -> RpcResult<talk::TalkId>;
    /// Stop talk `id`; `TalkFinished` follows as an event. Idempotent. Any
    /// authenticated peer may stop, including a relay peer.
    #[method(name = "talk_stop", with_extensions)]
    async fn talk_stop(&self, id: talk::TalkId) -> RpcResult<()>;
    /// Feed a `sink: "client"` talk (76 §5.3): `pcm` is base64 of little-endian
    /// mono i16 samples at the rate given to `talk_start`, ~100 ms per call;
    /// `played_seq` is the highest `SpeechAudio.seq` whose playback ended on
    /// the client since the last call — the heard-prefix position.
    #[method(name = "talk_audio", with_extensions)]
    async fn talk_audio(
        &self,
        id: talk::TalkId,
        pcm: String,
        played_seq: Option<u32>,
    ) -> RpcResult<()>;
    /// The running talk, if any, and the streaming model's state (76 §5.1).
    /// Open to relay peers.
    #[method(name = "talk_status", with_extensions)]
    async fn talk_status(&self) -> RpcResult<talk::TalkState>;
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
    async fn meeting_edit(
        &self,
        id: meeting::MeetingId,
        ops: Vec<meeting::EditOp>,
    ) -> RpcResult<meeting::Transcript>;
    /// `[t0, t1)` of one track as a base64 16-bit mono WAV (48 kHz), for the review pane's player.
    #[method(name = "meeting_segment_audio")]
    async fn meeting_segment_audio(
        &self,
        id: meeting::MeetingId,
        track: meeting::Track,
        t0_ms: u64,
        t1_ms: u64,
    ) -> RpcResult<String>;
    /// Enrolled speaker profiles (embedding omitted).
    #[method(name = "meeting_speakers")]
    async fn meeting_speakers(&self) -> RpcResult<Vec<meeting::Profile>>;
    /// Delete a speaker profile file. Transcripts keep their labels.
    #[method(name = "meeting_speaker_delete")]
    async fn meeting_speaker_delete(&self, profile: meeting::ProfileId) -> RpcResult<()>;
    /// 07 §6: the `meeting.live_diarization` switch as configured (default `true`).
    #[method(name = "meeting_live_diarization")]
    async fn meeting_live_diarization(&self) -> RpcResult<bool>;
    /// Persist `meeting.live_diarization` and apply it from the next meeting start; a
    /// running meeting keeps its mode. A failed config write changes nothing.
    #[method(name = "set_meeting_live_diarization")]
    async fn set_meeting_live_diarization(&self, on: bool) -> RpcResult<bool>;
    /// `Review` → `Summarizing`: the guided summary turn; `Summarized{title}` and `State{Done}` follow.
    #[method(name = "meeting_summarize")]
    async fn meeting_summarize(&self, id: meeting::MeetingId) -> RpcResult<()>;
    /// Re-run the post pipeline from the Opus files.
    #[method(name = "meeting_post_retry")]
    async fn meeting_post_retry(&self, id: meeting::MeetingId) -> RpcResult<()>;
    /// `summary.md`.
    #[method(name = "meeting_summary")]
    async fn meeting_summary(&self, id: meeting::MeetingId) -> RpcResult<String>;
    /// 57 §3.4 / §7.1: the decision-oracle switch of the families under `prefix` (`meeting`,
    /// `browser`) — `mode` as configured, `effective` (`off` with `reason` when the plugin is
    /// disabled or the checkpoint is missing), `enabled`, `installed`, and the tier budgets.
    #[method(name = "decision_mode")]
    async fn decision_mode(&self, prefix: String) -> RpcResult<decision::ModeInfo>;
    /// Persist `decision.families.<prefix>: { mode, train, share }` (`off` | `shadow` |
    /// `active`) and apply it live; effective from the next ask. A failed config write changes
    /// nothing. `browser` off ↔ on also changes whether `browser_run` is registered — that
    /// part takes a restart.
    #[method(name = "set_decision_mode")]
    async fn set_decision_mode(
        &self,
        prefix: String,
        mode: decision::Mode,
    ) -> RpcResult<decision::ModeInfo>;
    /// 126 §13: the double's status — enabled, paused, active version, last run, next window,
    /// venue, overall KPIs vs the frozen baseline.
    #[method(name = "double_status")]
    async fn double_status(&self) -> RpcResult<double::DoubleStatus>;
    /// Every discovered class with its rung, cap and last-30-days numbers (126 §13).
    #[method(name = "double_classes")]
    async fn double_classes(&self) -> RpcResult<Vec<double::DoubleClass>>;
    /// One class: the learning curve, the last 50 scrubbed examples, the promote hint (126 §13).
    #[method(name = "double_class")]
    async fn double_class(&self, key: String) -> RpcResult<double::DoubleClassDetail>;
    /// Start a training run now (126 §8): `tier` a | b | c, `venue` auto | local | node | off.
    /// Returns the run id; `DoubleTrainingState` events follow.
    #[method(name = "double_train")]
    async fn double_train(
        &self,
        tier: double::Tier,
        venue: double::Venue,
    ) -> RpcResult<double::RunId>;
    /// Pause / resume learning (126 §9): predictions and rungs continue, labels stop accruing.
    #[method(name = "double_pause")]
    async fn double_pause(&self, on: bool) -> RpcResult<double::DoubleStatus>;
    /// Forget a class's examples or specific examples (126 §14); returns the count removed.
    #[method(name = "double_forget")]
    async fn double_forget(&self, target: double::ForgetTarget) -> RpcResult<u32>;
    /// Make `version` the active one (126 §8.5); the next prediction uses it.
    #[method(name = "double_rollback")]
    async fn double_rollback(&self, version: String) -> RpcResult<double::DoubleStatus>;
    /// Set a class's cap and mutating opt-in (126 §9 caps); destructive stays ≤ 1.
    #[method(name = "double_set_cap")]
    async fn double_set_cap(
        &self,
        class: String,
        rung: u8,
        mutating_opt_in: bool,
    ) -> RpcResult<double::DoubleClass>;
    /// Write a scrubbed JSONL + embeddings archive under `<data_dir>/double/export/` (126 §14).
    #[method(name = "double_export")]
    async fn double_export(&self) -> RpcResult<double::DoubleExport>;
    /// Remove the store, models and runs; config untouched (126 §14).
    #[method(name = "double_delete_all")]
    async fn double_delete_all(&self) -> RpcResult<()>;
    /// Cancel an L2 grace so the owner answers through the ordinary `resolve_approval` /
    /// `answer_ask` (126 §9); `true` when a grace was running.
    #[method(name = "double_take_over")]
    async fn double_take_over(&self, question_id: String) -> RpcResult<bool>;
    /// The `double:` section as running (126 §12).
    #[method(name = "double_settings_get")]
    async fn double_settings_get(&self) -> RpcResult<double::DoubleConfig>;
    /// Persist the `double:` section and apply it live; `enabled` and `model.*` take a restart.
    #[method(name = "double_settings_set")]
    async fn double_settings_set(
        &self,
        settings: double::DoubleConfig,
    ) -> RpcResult<double::DoubleConfig>;
    /// 136 §2.7: dreaming's status — enabled, the last night, the next window, budget left tonight,
    /// backlog, proposals pending, paused_until. Answers from `state.json` even when the plugin is
    /// disabled (`enabled: false`, defaults otherwise).
    #[method(name = "dreaming.status")]
    async fn dreaming_status(&self) -> RpcResult<dreaming::DreamStatus>;
    /// The last `limit` nights, newest first (136 §2.7).
    #[method(name = "dreaming.nights")]
    async fn dreaming_nights(&self, limit: u32) -> RpcResult<Vec<dreaming::NightSummary>>;
    /// Dream now (136 §2.7): fire the routine's session with this run's budget and mode; returns the session.
    #[method(name = "dreaming.run_now")]
    async fn dreaming_run_now(&self, request: dreaming::RunRequest) -> RpcResult<SessionId>;
    /// Undo a night or one group of it (136 §2.7).
    #[method(name = "dreaming.undo")]
    async fn dreaming_undo(
        &self,
        night: String,
        group: Option<String>,
    ) -> RpcResult<dreaming::UndoReport>;
    /// Settings › Dreaming as running (136 §2.7); from the config section even when disabled.
    #[method(name = "dreaming.settings")]
    async fn dreaming_settings(&self) -> RpcResult<dreaming::DreamSettings>;
    /// Persist the `dreaming:` section through the settings path and apply it live (136 §2.7).
    #[method(name = "dreaming.set_settings")]
    async fn dreaming_set_settings(
        &self,
        settings: dreaming::DreamSettings,
    ) -> RpcResult<dreaming::DreamSettings>;
    /// Proposals waiting for the user (136 §2.7).
    #[method(name = "dreaming.proposals")]
    async fn dreaming_proposals(&self) -> RpcResult<Vec<dreaming::Proposal>>;
    /// The user pressed `choice` on a proposal (136 §2.7).
    #[method(name = "dreaming.proposal_act")]
    async fn dreaming_proposal_act(&self, id: String, choice: String) -> RpcResult<()>;
    /// The 136 §13 series over `days`.
    #[method(name = "dreaming.metrics")]
    async fn dreaming_metrics(&self, days: u32) -> RpcResult<dreaming::Metrics>;
    /// 137 §11.1 `agent.endless` as in force (Settings › Memory).
    #[method(name = "endless_settings_get")]
    async fn endless_settings_get(&self) -> RpcResult<EndlessConfig>;
    /// Persist the whole `agent.endless` block to config.yaml and apply it
    /// live (defaults for new chats/desks/meetings, view budget, slack,
    /// settle). The compactor model applies to sessions attached after the
    /// change. A failed write changes nothing.
    #[method(name = "endless_settings_set")]
    async fn endless_settings_set(&self, config: EndlessConfig) -> RpcResult<EndlessConfig>;
    /// Settings › Search (16 §8): the four external providers (enabled, base
    /// URL, secret name and whether it is set - never the value, timeout,
    /// priority, registered) plus the `web` read/budget defaults.
    #[method(name = "search_settings_get")]
    async fn search_settings_get(&self) -> RpcResult<search_providers::SearchSettings>;
    /// Persist `search_providers.*` and the `web` defaults to config.yaml and
    /// apply them live: providers are re-registered with the `web` actor, the
    /// next `search_web` uses the new limits. A failed write changes nothing.
    #[method(name = "search_settings_set")]
    async fn search_settings_set(
        &self,
        settings: search_providers::SearchSettings,
    ) -> RpcResult<search_providers::SearchSettings>;
    /// One probe query through one registered provider (built-in or external):
    /// ok, latency, rows, error text. Unknown / unregistered names are errors.
    #[method(name = "search_provider_test")]
    async fn search_provider_test(
        &self,
        name: String,
        query: String,
    ) -> RpcResult<web::settings::ProviderProbe>;
    /// Per-provider health from the last calls (rate-limited, captcha, parse
    /// error, …) for every registered provider, in registration order.
    #[method(name = "search_status")]
    async fn search_status(&self) -> RpcResult<Vec<web::health::ProviderStatus>>;
    /// Run the whole `search_web` tool once (fan-out, rank, read) from Settings
    /// › Search "Run a test search"; returns the model-facing text and the JSON.
    #[method(name = "search_test")]
    async fn search_test(
        &self,
        query: String,
        site: Option<String>,
        recent: Option<web::Recency>,
    ) -> RpcResult<web::settings::SearchTestResult>;
    /// `decision_mode("meeting")`.
    #[method(name = "meeting_oracle_mode")]
    async fn meeting_oracle_mode(&self) -> RpcResult<decision::ModeInfo>;
    /// `set_decision_mode("meeting", mode)`.
    #[method(name = "set_meeting_oracle_mode")]
    async fn set_meeting_oracle_mode(&self, mode: decision::Mode) -> RpcResult<decision::ModeInfo>;
    /// 57 §7: today's decision log folded per family — mode, asks, accepts, escalations by
    /// reason, tier-2 rows, p50/p95, shadow agreement.
    #[method(name = "decision_stats")]
    async fn decision_stats(&self) -> RpcResult<Vec<decision::FamilyStats>>;
    /// 57 §2.1 item 4: the decision checkpoints resident right now — `[]` at idle. `rss_mb`
    /// is the whole daemon's footprint, repeated on every entry.
    #[method(name = "decision_loaded")]
    async fn decision_loaded(&self) -> RpcResult<Vec<decision::LoadedInfo>>;
    /// 57 §11.4 / §7.1: whether the families under `prefix` (`meeting`, `browser`) capture
    /// training examples locally (`enabled`), consent to upload them (`share`), and how much is
    /// on disk (`<data_dir>/decisions/training/<prefix>/`).
    #[method(name = "decision_training")]
    async fn decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo>;
    /// Persist `decision.families.<prefix>: { mode, train, share }` and apply it live; effective
    /// from the next batch. Turning capture off also clears `share`. A failed config write
    /// changes nothing.
    #[method(name = "set_decision_training")]
    async fn set_decision_training(
        &self,
        prefix: String,
        on: bool,
    ) -> RpcResult<decision::TrainingInfo>;
    /// 57 §7.1: consent to upload the prefix's captured rows, persisted the same way. Refused
    /// (`-32000`) when capture is off. No uploader reads it in this version.
    #[method(name = "set_decision_share")]
    async fn set_decision_share(
        &self,
        prefix: String,
        on: bool,
    ) -> RpcResult<decision::TrainingInfo>;
    /// 57 §7.1: remove `<data_dir>/decisions/training/<prefix>/`; the switches stay as set.
    /// Answers the new counts.
    #[method(name = "delete_decision_training")]
    async fn delete_decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo>;
    /// Phase 5 (05 §8): the brief cache. Full-text search, best rank first (`limit` ≤ 50, default 8).
    #[method(name = "brief_search")]
    async fn brief_search(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<meeting::CachedBrief>>;
    /// Cached briefs, newest `updated_at` first, optionally only those tagged `tag` (`limit` ≤ 50).
    #[method(name = "brief_list")]
    async fn brief_list(
        &self,
        tag: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<meeting::CachedBrief>>;
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
    /// A schedule file's front matter and body for the editor. `Err` when unknown.
    #[method(name = "get_schedule")]
    async fn get_schedule(&self, name: String) -> RpcResult<schedule::file::ScheduleFile>;
    /// Validate and write a schedule file (create or replace); returns its live row.
    /// `Err` with the name rule, the parse error, or the io error.
    #[method(name = "put_schedule")]
    async fn put_schedule(
        &self,
        file: schedule::file::ScheduleFile,
    ) -> RpcResult<schedule::ScheduleInfo>;
    /// Remove a schedule file; its history gets a `removed` row. `Err` when unknown.
    #[method(name = "delete_schedule")]
    async fn delete_schedule(&self, name: String) -> RpcResult<()>;
    /// The 40 §1.12 verdict for a `when:` schedule: dry-run its sensor twice,
    /// probe, report `fire?`, hash, flaps, "would fire now". `Err` when unknown
    /// or not a watcher.
    #[method(name = "test_schedule")]
    async fn test_schedule(&self, name: String) -> RpcResult<schedule::Verdict>;
    /// The 40 §1.12 verdict on an unsaved file (108 §9.4): render + parse +
    /// dry-run, nothing written. `Err` with the parse error or `not a watcher`.
    #[method(name = "test_schedule_file")]
    async fn test_schedule_file(
        &self,
        file: schedule::file::ScheduleFile,
    ) -> RpcResult<schedule::Verdict>;
    /// This engine, its peers and the hub lease (67 §4, §12).
    #[method(name = "mesh_status")]
    async fn mesh_status(&self) -> RpcResult<mesh::MeshStatus>;
    /// A pairing string another engine can `t mesh add` (67 §4.1).
    #[method(name = "mesh_pairing_string")]
    async fn mesh_pairing_string(&self) -> RpcResult<String>;
    /// Pair with the engine behind `pairing`; mutual and transitive (67 §4.1).
    #[method(name = "mesh_add")]
    async fn mesh_add(&self, pairing: String) -> RpcResult<mesh::Peer>;
    /// Leave an engine out of the mesh (67 §4.1).
    #[method(name = "mesh_remove")]
    async fn mesh_remove(&self, id: String) -> RpcResult<()>;
    /// This engine's hub lease preference (67 §4.5); written to `mesh.hub`.
    #[method(name = "mesh_set_hub")]
    async fn mesh_set_hub(&self, pref: mesh::HubPref) -> RpcResult<()>;
    /// Every sandbox record (docker, ssh, vm) with state, resources, shares, metrics (79 §4.5).
    #[method(name = "sandboxes_list")]
    async fn sandboxes_list(&self) -> RpcResult<Vec<sandbox::SandboxInfo>>;
    /// Host readiness for a sandbox VM: runtime, virtualization, derived defaults, blockers (79 §5.1).
    #[method(name = "sandbox_preflight")]
    async fn sandbox_preflight(&self) -> RpcResult<sandbox::Preflight>;
    /// Install msb + libkrunfw; progress as `SandboxEvent::Progress { name: "runtime" }`.
    #[method(name = "sandbox_install_runtime")]
    async fn sandbox_install_runtime(&self) -> RpcResult<()>;
    /// Create a sandbox VM; returns at once in `creating`, bring-up streams `Progress` (79 §5.2).
    #[method(name = "sandbox_create")]
    async fn sandbox_create(&self, spec: sandbox::SandboxCreate)
    -> RpcResult<sandbox::SandboxInfo>;
    /// Resume a `failed` bring-up from its step.
    #[method(name = "sandbox_retry")]
    async fn sandbox_retry(&self, name: String) -> RpcResult<sandbox::SandboxInfo>;
    /// Boot a stopped VM (pending shares, ports and ceilings apply).
    #[method(name = "sandbox_start")]
    async fn sandbox_start(&self, name: String) -> RpcResult<sandbox::SandboxInfo>;
    /// Stop gracefully for `grace_secs` (20), then kill; `force` skips the busy-sessions warning.
    #[method(name = "sandbox_stop")]
    async fn sandbox_stop(
        &self,
        name: String,
        grace_secs: Option<u64>,
        force: Option<bool>,
    ) -> RpcResult<sandbox::SandboxInfo>;
    /// Pause (RAM kept).
    #[method(name = "sandbox_pause")]
    async fn sandbox_pause(&self, name: String) -> RpcResult<sandbox::SandboxInfo>;
    /// Resume a paused VM.
    #[method(name = "sandbox_resume")]
    async fn sandbox_resume(&self, name: String) -> RpcResult<sandbox::SandboxInfo>;
    /// Change CPUs / memory / disk / ceilings; live within ceilings (79 §5.5).
    #[method(name = "sandbox_resize")]
    async fn sandbox_resize(
        &self,
        name: String,
        change: sandbox::ResourceChange,
    ) -> RpcResult<sandbox::ResizeOutcome>;
    /// Replace the share list; applied by the next start (79 §5.6).
    #[method(name = "sandbox_set_shares")]
    async fn sandbox_set_shares(
        &self,
        name: String,
        shares: Vec<sandbox::Mount>,
    ) -> RpcResult<sandbox::Pending>;
    /// Take a disk (or `full`: memory too) snapshot.
    #[method(name = "sandbox_snapshot")]
    async fn sandbox_snapshot(
        &self,
        name: String,
        label: String,
        full: Option<bool>,
    ) -> RpcResult<sandbox::VmSnapshotInfo>;
    /// Restore a snapshot; the current disk is discarded.
    #[method(name = "sandbox_restore")]
    async fn sandbox_restore(
        &self,
        name: String,
        id: sandbox::SnapshotId,
    ) -> RpcResult<sandbox::SandboxInfo>;
    /// Remove peer, VM, vault row and record; snapshots kept unless `keep_snapshots: false`.
    #[method(name = "sandbox_destroy")]
    async fn sandbox_destroy(&self, name: String, keep_snapshots: Option<bool>) -> RpcResult<()>;
    /// noVNC endpoint and password for the pop-out (RPC only, never a tool result; 79 §6.2).
    #[method(name = "sandbox_desktop_access")]
    async fn sandbox_desktop_access(
        &self,
        name: String,
    ) -> RpcResult<sandbox::types::DesktopAccess>;
    /// Live CPU / memory / disk / network numbers.
    #[method(name = "sandbox_metrics")]
    async fn sandbox_metrics(&self, name: String) -> RpcResult<sandbox::VmMetrics>;
    /// Last `tail` (200) runtime log lines.
    #[method(name = "sandbox_logs")]
    async fn sandbox_logs(
        &self,
        name: String,
        tail: Option<usize>,
    ) -> RpcResult<Vec<sandbox::LogLine>>;
    /// The user takes (`on`) or hands back the VM's desktop; steers every running session on it (79 §6.5). Returns sessions steered.
    #[method(name = "sandbox_takeover")]
    async fn sandbox_takeover(&self, name: String, on: bool) -> RpcResult<u32>;
    /// Open an interactive shell in the VM as a PTY job of `session` (the caller's; an owner
    /// root is opened when absent). Returns `{ job, session }`; attach with `subscribe_pty`,
    /// type with `pty_input`, resize with `pty_resize`.
    #[method(name = "sandbox_terminal", with_extensions)]
    async fn sandbox_terminal(
        &self,
        name: String,
        session: Option<SessionId>,
    ) -> RpcResult<serde_json::Value>;
    /// Unresolved desktop hand-offs (79 §6.6).
    #[method(name = "handoffs_list")]
    async fn handoffs_list(&self) -> RpcResult<Vec<roster::attention::Handoff>>;
    /// Resolve a hand-off: `outcome` is `done` | `cancelled`.
    #[method(name = "handoff_resolve")]
    async fn handoff_resolve(&self, id: String, outcome: String) -> RpcResult<()>;
    /// Every roster agent with status, desk, open tasks and month spend (67 §12).
    #[method(name = "agents_list")]
    async fn agents_list(&self) -> RpcResult<Vec<roster::AgentSummary>>;
    /// One agent file (67 §6.1). `Err` when unknown.
    #[method(name = "agent_get")]
    async fn agent_get(&self, name: String) -> RpcResult<roster::AgentFile>;
    /// The agent's desk session id, opening the desk when it is not (the Desks row's click). `Err` when the agent is unknown, paused, a proposal, or this engine is not the hub.
    #[method(name = "desk_open")]
    async fn desk_open(&self, name: String) -> RpcResult<SessionId>;
    /// Write `agents/<name>.md` from `file` (create or replace) — the file editor's save (67 §12).
    #[method(name = "agent_put")]
    async fn agent_put(&self, file: roster::AgentFile) -> RpcResult<()>;
    /// Flip an agent's `paused:` line; paused agents receive no wakes (67 §6.4).
    #[method(name = "agent_pause")]
    async fn agent_pause(&self, name: String, paused: bool) -> RpcResult<()>;
    /// Approve a proposed agent: clears `proposed_by`, unpauses (67 §6.5).
    #[method(name = "agent_approve", with_extensions)]
    async fn agent_approve(&self, name: String) -> RpcResult<()>;
    /// Delete `agents/<name>.md`; a pending proposal is refused with `note` on its hire task (67 §6.5). `theuth` is refused.
    #[method(name = "agent_remove")]
    async fn agent_remove(&self, name: String, note: Option<String>) -> RpcResult<()>;
    /// Tasks matching `filter`, oldest first (67 §12 board).
    #[method(name = "tasks_list")]
    async fn tasks_list(&self, filter: tasks::TaskFilter) -> RpcResult<Vec<tasks::Task>>;
    /// One task (67 §7.1). `Err` when unknown.
    #[method(name = "task_get")]
    async fn task_get(&self, id: String) -> RpcResult<tasks::Task>;
    /// Create a task as the user (`created_by: user`); the owner is woken (67 §7).
    #[method(name = "task_create")]
    async fn task_create(&self, input: tasks::TaskCreate) -> RpcResult<tasks::Task>;
    /// Patch a task as the user: assign, set state, blockers, placement (67 §7.2, §12).
    #[method(name = "task_update")]
    async fn task_update(&self, id: String, patch: tasks::TaskPatch) -> RpcResult<tasks::Task>;
    /// Move a task on the user's intent (67 §7.2): backlog ↔ todo, → cancelled,
    /// review → done | todo, blocked → todo. `Err` names the allowed targets.
    /// `next` (132 D3): with `done` on a review that offered next moves, the
    /// index of the pick — `**next** (n) <label>` in the thread, the owner woken `answer`.
    #[method(name = "task_move", with_extensions)]
    async fn task_move(
        &self,
        id: String,
        state: tasks::TaskState,
        note: Option<String>,
        next: Option<u32>,
    ) -> RpcResult<tasks::Task>;
    /// Append a comment to a task's thread as the user; wakes the owner (67 §7.3).
    #[method(name = "task_comment")]
    async fn task_comment(&self, id: String, text: String) -> RpcResult<()>;
    /// The task's field-level event log, oldest first (67 §7.2).
    #[method(name = "task_events")]
    async fn task_events(&self, id: String) -> RpcResult<Vec<tasks::TaskEvent>>;
    /// Tasks whose `blockers` name `id` (the inverse link, computed; 67 §7.1).
    #[method(name = "task_blocks")]
    async fn task_blocks(&self, id: String) -> RpcResult<Vec<tasks::TaskId>>;
    /// The activity feed, newest first: `since_ms`, `kinds`, `limit` (default 50, max 500) (70 HUD).
    #[method(name = "activity_list")]
    async fn activity_list(
        &self,
        filter: tasks::activity::ActivityFilter,
    ) -> RpcResult<Vec<tasks::activity::ActivityEntry>>;
    /// The attention inbox (70 HUD): approvals, decisions, hires, blocked / review tasks,
    /// budgets, unreachable nodes — undismissed and unsnoozed, newest first. Badge = length.
    #[method(name = "attention_list")]
    async fn attention_list(&self) -> RpcResult<Vec<roster::attention::AttentionItem>>;
    /// Hide an attention item until `until_ms` (ms since epoch).
    #[method(name = "attention_snooze")]
    async fn attention_snooze(&self, id: String, until_ms: u64) -> RpcResult<()>;
    /// Hide an attention item for good.
    #[method(name = "attention_dismiss")]
    async fn attention_dismiss(&self, id: String) -> RpcResult<()>;
    /// 109 §10.3: resume `session`'s open turn now (the `resume: ask` mode's
    /// Resume button). `false` when the session has no open turn.
    #[method(name = "resume_turn", with_extensions)]
    async fn resume_turn(&self, session: SessionId) -> RpcResult<bool>;
    /// The packet behind a decision / review attention item (101 §3): `decision:<id>` | `review:<task>`;
    /// `null` when none was raised.
    #[method(name = "packet_get")]
    async fn packet_get(&self, key: String) -> RpcResult<Option<tasks::packet::Packet>>;
    /// Answer the `decision:<id>` a task is blocked on, as the user (67 §11); the owner is re-woken.
    #[method(name = "task_decide", with_extensions)]
    async fn task_decide(&self, decision: String, answer: String) -> RpcResult<()>;
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
    /// Publish a reply fence's OpenUI source as an app artifact of `session` without a
    /// model turn (30 §9): validated and committed exactly like the `app` tool with
    /// `edit: false`, under a synthetic `ui-<uuid v7>` call id, no transcript entry. The
    /// error message is the tool's located diagnostics text.
    #[method(name = "publish_app")]
    async fn publish_app(
        &self,
        session: SessionId,
        name: String,
        source: String,
    ) -> RpcResult<AppDoc>;
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
    /// The member listing of a `Kind::Archive` record (35 §3.2): paths,
    /// sizes and dates from the archive's headers, at most 5,000 entries
    /// (`truncated` past that). Rejects when the record vanished or the
    /// archive was unreadable at show time (no listing sidecar).
    #[method(name = "get_artifact_listing")]
    async fn get_artifact_listing(
        &self,
        call_id: String,
    ) -> RpcResult<artifact::archive::ArchiveListing>;
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
    /// Answer a `SecretsEvent::Requested` on this or any engine (79 §6.6): `value` for a
    /// single field, `values` (`{ name: value }`) for a form, both `None` declines. Stored as
    /// `source: user`. `true` if it was still unanswered.
    #[method(name = "answer_secret")]
    async fn answer_secret(
        &self,
        id: secrets::RequestId,
        value: Option<String>,
        values: Option<HashMap<String, String>>,
    ) -> RpcResult<bool>;
    /// The unanswered replicated `secret_requests` rows, newest first (79 §6.6): what the
    /// attention inbox's `secret:<id>` items render (`title`, `fields`) and answer with
    /// `answer_secret` from any engine.
    #[method(name = "secret_requests_list")]
    async fn secret_requests_list(&self) -> RpcResult<Vec<secrets::SecretRequest>>;
    /// 129 §3.2: per-connection focus. Delta-class events (`TextDelta`, `ReasoningDelta`,
    /// `ToolCallProgress`, `VisionProgress`) reach this connection's `subscribe_events` streams
    /// only for `sessions`; every other event is unaffected. A connection that never calls this
    /// receives everything. Replaces the previous set; `[]` means no deltas at all. The set dies
    /// with the connection.
    #[method(name = "set_event_focus", with_extensions)]
    async fn set_event_focus(&self, sessions: Vec<SessionId>) -> RpcResult<()>;
    /// Stream every wire-visible event as `{"type": ..., ...}`, optionally
    /// only those carrying `session`. `descendants: true` (default `false`)
    /// also delivers events of every session in `session`'s subtree, plus
    /// `SubagentSpawned` whose `parent` is in it (19 §2.2). The first item is
    /// always a [`PendingPrompts`] snapshot; a client replaces its pending
    /// prompt set with it. A slow client sees `{"type":"Lagged","count":n}`
    /// where `n` non-delta events were dropped for it (129 §3.3), followed by
    /// a fresh snapshot; delta-class events are merged while they wait and
    /// are never lost that way. Deltas of sessions outside `set_event_focus`
    /// are not delivered.
    #[subscription(name = "subscribe_events", unsubscribe = "unsubscribe_events", item = serde_json::Value, with_extensions)]
    async fn subscribe_events(
        &self,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult;
}

/// The first item of every `subscribe_events` stream: every approval,
/// `ask_user` and `secret_request` still waiting on a user, filtered like the
/// live events. A client subscribing (or reconnecting over the relay) after
/// the `*Requested` broadcast still gets the card, and one that missed the
/// `*Resolved` drops it: after this item the client's pending set is exactly
/// what the engine reports (31 §7.3).
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct PendingPrompts {
    /// Pending approvals.
    pub approvals: Vec<ApprovalRequested>,
    /// Pending `ask_user` calls.
    pub asks: Vec<AskRequested>,
    /// Pending `secret_request` calls (`SecretRequested` variants only).
    pub secrets: Vec<secrets::SecretsEvent>,
}

impl PendingPrompts {
    /// The engine's pending prompts that `keep` admits (one item at a time, as
    /// its `{"type": …}` JSON). An actor that is not running contributes none.
    pub async fn collect(mut keep: impl FnMut(&serde_json::Value) -> bool) -> Self {
        let mut out = Self::default();
        fn admit<T: Serialize>(
            items: Vec<T>,
            keep: &mut impl FnMut(&serde_json::Value) -> bool,
        ) -> Vec<T> {
            items
                .into_iter()
                .filter(|i| serde_json::to_value(i).is_ok_and(|v| keep(&v)))
                .collect()
        }
        if let Ok(Some(a)) = ActorRef::<Approval>::lookup(tools::APPROVAL_ACTOR)
            && let Ok(v) = a.ask(tools::PendingApprovals).await
        {
            out.approvals = admit(v, &mut keep);
        }
        if let Ok(Some(a)) = ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR)
            && let Ok(v) = a.ask(tools::PendingAsks).await
        {
            out.asks = admit(v, &mut keep);
        }
        if let Ok(Some(a)) = ActorRef::<secrets::SecretsActor>::lookup(secrets::SECRETS_ACTOR)
            && let Ok(v) = a.ask(secrets::PendingRequests).await
        {
            out.secrets = admit(v, &mut keep);
        }
        out
    }
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

/// 129 §3.2: `connection id → sessions` whose delta-class events the connection
/// wants, as the strings the serialized `session` field carries. No entry means
/// everything; the entry is removed when the connection closes.
pub type FocusMap = Arc<RwLock<HashMap<u64, Arc<HashSet<String>>>>>;

/// Server state: the event feed and the runtime (for `shutdown`).
pub struct Server {
    /// Feed from [`crate::bridge`].
    pub events: broadcast::Sender<Event>,
    /// `rt.shutdown` is cancelled by `shutdown`.
    pub rt: Runtime,
    /// Private-tree membership for admission of events; see [`PrivateMap`].
    pub private: PrivateMap,
    /// Per-connection delta focus; see [`FocusMap`].
    pub focus: FocusMap,
}

impl Server {
    /// A server with no event feed: for in-process callers (the TUI), which
    /// subscribe to the bus themselves. `serve` builds the real one.
    pub fn new(rt: Runtime) -> Self {
        Self {
            events: broadcast::channel(1).0,
            rt,
            private: PRIVATE.clone(),
            focus: Default::default(),
        }
    }

    /// The `double:` section as read at start (126 §12); defaults when absent.
    fn double_config(&self) -> double::DoubleConfig {
        self.rt.section("double").unwrap_or_default()
    }

    /// The `dreaming:` section as read at start (136 §9.2); defaults when absent.
    fn dreaming_config(&self) -> dreaming::DreamConfig {
        self.rt.section("dreaming").unwrap_or_default()
    }
}

impl Server {
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

/// A `dreaming::rpc` error → JSON-RPC: "not running" is `NOT_RUNNING`, anything else `ASK_FAILED`.
fn dreaming_err(e: String) -> ErrorObjectOwned {
    let code = if e.ends_with("is not running") {
        NOT_RUNNING
    } else {
        ASK_FAILED
    };
    ErrorObjectOwned::owned(code, e, None::<()>)
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

/// 139 §6.1 (D16): the supervisor refuses an input to a session mid-release-drain
/// with a handler error whose `Display` form is `SessionDraining:<reason>[:<next owner>]`
/// (`agent::supervisor::transfer::DRAINING_PREFIX`, the `DocError::NotHub` convention).
/// Mapped here to the retryable [`forward::SESSION_DRAINING`] with the typed payload;
/// the composer retries once. Anything else passes through.
fn session_draining(session: SessionId, e: ErrorObjectOwned) -> ErrorObjectOwned {
    let Some(rest) = e.message().strip_prefix("SessionDraining:") else {
        return e;
    };
    let (reason, next_owner) = match rest.split_once(':') {
        Some((r, o)) if !o.is_empty() => (r.to_owned(), Some(o.to_owned())),
        Some((r, _)) => (r.to_owned(), None),
        None => (rest.to_owned(), None),
    };
    let msg = match (reason.as_str(), &next_owner) {
        ("release", Some(to)) => format!("{session} is moving to {to} — retry in a moment"),
        ("release", None) => format!("{session} is moving to another engine — retry in a moment"),
        (r, _) => format!("{session} is pausing for {r} — resumes on the next start"),
    };
    ErrorObjectOwned::owned(
        forward::SESSION_DRAINING,
        msg,
        Some(forward::FrontDoorError::SessionDraining {
            session,
            reason,
            next_owner,
        }),
    )
}

/// 139 §9.2 (D18): the engine owning a question's session, when it is another
/// one — read from the replicated `questions` row, so an answer typed on any
/// engine finds the actor holding the oneshot. `None`: no row (a pre-0033
/// question or a non-journaled kind), or the session is unknown / ours.
async fn question_owner(
    store: &agent::store::Store,
    id: &Uuid,
) -> RpcResult<Option<replica::PeerId>> {
    let Some(row) = store
        .question_get(&id.to_string())
        .await
        .map_err(|e| failed(format!("{e:#}")))?
    else {
        return Ok(None);
    };
    let Some((owner, _)) = store
        .owner(row.session)
        .await
        .map_err(|e| failed(format!("{e:#}")))?
    else {
        return Ok(None);
    };
    Ok((!owner.is_empty() && owner != store.engine()).then_some(replica::PeerId(owner)))
}

/// 139 §9.2: `answer_ask` / `resolve_approval` to the owner of the question's
/// session — forward, never take: an answer must reach the actor holding the
/// oneshot, and a take would kill it. `Ok(None)`: answer locally (lone engine,
/// already forwarded once, no row, or the owner is here). Owner not connected
/// → [`forward::OWNER_UNREACHABLE`], the card the desktop already renders.
async fn route_question<T: DeserializeOwned>(
    ext: &Extensions,
    id: &Uuid,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    if ext.get::<forward::Forwarded>().is_some() || sessionsync::actor().is_none() {
        return Ok(None);
    }
    let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
    route_question_via(ext, &store, id, method, params).await
}

/// [`route_question`] past the plugin gate, over a given store (testable without actors).
async fn route_question_via<T: DeserializeOwned>(
    ext: &Extensions,
    store: &agent::store::Store,
    id: &Uuid,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    let Some(owner) = question_owner(store, id).await? else {
        return Ok(None);
    };
    match forward::to_peer(ext, &owner, method, params).await? {
        Some(v) => Ok(Some(v)),
        None => Err(forward::unreachable(owner.0, "not connected").await),
    }
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

/// The mesh actor; absent when the plugin is not loaded or its store failed (67 §4).
fn mesh_actor() -> RpcResult<ActorRef<mesh::Mesh>> {
    lookup::<mesh::Mesh>(mesh::MESH_ACTOR)
}

/// The roster actor; absent when the plugin is not loaded or its store failed (67 §6).
fn roster_actor() -> RpcResult<ActorRef<roster::Roster>> {
    lookup::<roster::Roster>(roster::ROSTER_ACTOR)
}

/// The tasks actor; absent when the plugin is not loaded or its store failed (67 §7).
fn tasks_actor() -> RpcResult<ActorRef<tasks::Tasks>> {
    lookup::<tasks::Tasks>(tasks::TASKS_ACTOR)
}

/// The attention actor; absent when the roster plugin is not loaded (70 HUD).
fn attention_actor() -> RpcResult<ActorRef<roster::attention::Attention>> {
    lookup::<roster::attention::Attention>(roster::attention::ATTENTION_ACTOR)
}

/// The sandbox actor; absent when `sandbox.enabled: false` or the plugin is not loaded (79).
fn sandbox() -> RpcResult<ActorRef<sandbox::SandboxActor>> {
    lookup::<sandbox::SandboxActor>(sandbox::SANDBOX_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "sandbox is disabled", None::<()>),
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

/// The talk actor; absent when `talk.enabled: false` or the plugin is not loaded.
fn talk() -> RpcResult<ActorRef<talk::Talk>> {
    lookup::<talk::Talk>(talk::TALK_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "talk is disabled", None::<()>),
        _ => e,
    })
}

/// The telegram actor; absent when `telegram.enabled: false` or the plugin is not loaded.
fn telegram() -> RpcResult<ActorRef<telegram::Telegram>> {
    lookup::<telegram::Telegram>(telegram::TELEGRAM_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "telegram is disabled", None::<()>),
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

/// The document principal behind a call (59 §8.3): a mode-2 guest conn is `Member{pub, device}`,
/// everything else the owner on `device`. Returned with the peer so the caller can mark a guest
/// joined or refused.
fn principal(ext: &Extensions) -> RpcResult<(link::RpcPeer, document::Principal)> {
    let p = peer(ext)?;
    let device = Some(p.id());
    let by = match p.guest() {
        Some(g) => document::Principal::Member {
            pub_: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(g.key),
            device,
        },
        None => document::Principal::Owner { device },
    };
    Ok((p, by))
}

/// Owner-only methods (59 §8: `doc_members`, links): a guest peer is refused before the actor sees it.
fn owner(ext: &Extensions) -> RpcResult<link::RpcPeer> {
    let p = peer(ext)?;
    if p.guest().is_some() {
        return Err(ErrorObjectOwned::owned(
            UNAUTHORIZED,
            "owner only",
            None::<()>,
        ));
    }
    Ok(p)
}

/// A member call's outcome as the guest's conn sees it (59 §8.3, §11): success scopes the conn
/// (no `doc_join` needed on a return visit), a membership or role refusal counts toward the close.
fn member_outcome<T>(peer: &link::RpcPeer, r: &RpcResult<T>) {
    let Some(g) = peer.guest() else {
        return;
    };
    match r {
        Ok(_) => g.mark_joined(),
        Err(e)
            if e.message().starts_with("not_member")
                || e.message().starts_with("viewer_readonly")
                || e.message().starts_with("sharing_disabled") =>
        {
            g.refuse();
        }
        Err(_) => {}
    }
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

/// The document actor; absent when `documents.enabled: false` or the plugin is not loaded.
fn documents() -> RpcResult<ActorRef<document::DocumentActor>> {
    lookup::<document::DocumentActor>(document::DOCUMENT_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "documents are disabled", None::<()>),
        _ => e,
    })
}

/// JSON-RPC error code: this engine does not hold the hub lease, and the document call is a hub duty
/// (69 §5). `data` is a `document::DocError` in serde's external enum form:
/// `{"NotHub": {"hub": "<engine id>" | null}}` — Y's forwarding keys on it.
pub const NOT_HUB: i32 = -32011;

/// [`send`] for the document actor: a `NotHub` refusal (typed, or in its `Display` form from a
/// handler whose error stayed a `String`) becomes [`NOT_HUB`] with the typed payload; anything else
/// is `ASK_FAILED` with the message.
async fn doc_send<M>(
    actor: &ActorRef<document::DocumentActor>,
    msg: M,
) -> RpcResult<<<document::DocumentActor as Message<M>>::Reply as Reply>::Ok>
where
    document::DocumentActor: Message<M>,
    M: Send + 'static,
    <<document::DocumentActor as Message<M>>::Reply as Reply>::Error: Display,
{
    actor.ask(msg).await.map_err(|e: SendError<M, _>| match e {
        SendError::HandlerError(e) => match document::DocError::parse(&e.to_string()) {
            err @ document::DocError::NotHub { .. } => {
                ErrorObjectOwned::owned(NOT_HUB, "another engine holds the hub lease", Some(err))
            }
            document::DocError::Other(msg) => failed(msg),
        },
        other => failed(other.to_string()),
    })
}

/// 69 Y: a hub-only owner call this engine refused with `NotHub{hub}` is re-issued verbatim to `hub`
/// as one unary JSON-RPC over `theuth/rpc/1` (`forward::to_peer`; request/reply, so no stream). The
/// hub dispatches it as its internal peer: these are owner-only or owner-side methods, so the
/// principal is right there. `params` is the positional list in trait order. `hub: null`, no
/// transport, or a hub not connected → the refusal stands (the client sees `-32011`).
async fn doc_forward<T: DeserializeOwned>(
    ext: &Extensions,
    method: &str,
    params: impl Serialize,
    r: RpcResult<T>,
) -> RpcResult<T> {
    let Err(e) = &r else { return r };
    if e.code() != NOT_HUB {
        return r;
    }
    let hub = e
        .data()
        .and_then(|d| serde_json::from_str::<document::DocError>(d.get()).ok())
        .and_then(|d| d.not_hub().cloned().flatten());
    let Some(hub) = hub else { return r };
    match forward::to_peer(ext, &replica::PeerId(hub.0), method, params).await? {
        Some(v) => Ok(v),
        None => r,
    }
}

/// [`doc_send`]'s error mapping for the 144 delegation functions that answer `anyhow::Result`: a
/// `NotHub:<id>` message becomes [`NOT_HUB`] with the typed payload (so [`doc_forward`] re-issues the
/// call), anything else `ASK_FAILED` with the message.
fn nb_result<T>(r: anyhow::Result<T>) -> RpcResult<T> {
    r.map_err(|e| match document::DocError::parse(&format!("{e:#}")) {
        err @ document::DocError::NotHub { .. } => {
            ErrorObjectOwned::owned(NOT_HUB, "another engine holds the hub lease", Some(err))
        }
        document::DocError::Other(msg) => failed(msg),
    })
}

/// 144 §8 (W3 R1): `Notebook.chats` from the agent store (`sessions.notebook`); the document crate holds no
/// session rows. Best effort — no agent actor or a store error leaves the count at 0.
async fn notebook_chat_counts() -> std::collections::HashMap<String, u32> {
    match lookup::<Agents>(agent::AGENT_ACTOR) {
        Ok(agents) => agent::notebooks::chat_counts(&agents)
            .await
            .unwrap_or_default(),
        Err(_) => Default::default(),
    }
}

/// 144 §6.4 (W3 R1): the document crate answers `same`/available for `theuth://sessions/<id>#<turn>` unchecked;
/// the agent store decides — no such (readable) session → `available: false`, a turn past the end → `missing`;
/// the title becomes the session's display name. Best effort when the agent actor is not running.
async fn check_session_cites(cites: &mut [document::Citation]) {
    let Ok(agents) = lookup::<Agents>(agent::AGENT_ACTOR) else {
        return;
    };
    for c in cites
        .iter_mut()
        .filter(|c| c.kind == document::CiteKind::Sessions)
    {
        let Ok(anchor) = surface::anchor::TheuthAnchor::parse(&c.href) else {
            continue;
        };
        match agent::notebooks::session_cite(&agents, &anchor.id).await {
            Ok(Some(s)) => {
                c.title = s.title;
                if let Some(surface::anchor::Locator::Turn(t)) = anchor.locator
                    && t as usize > s.turns
                {
                    c.status = codeindex::LocateStatus::Missing;
                }
            }
            Ok(None) => c.available = false,
            Err(e) => tracing::debug!(href = %c.href, "session citation check skipped: {e:#}"),
        }
    }
}

/// The knowledge actor; `knowledge.enabled: false` means it was never
/// spawned (12 §9.3).
fn knowledge() -> RpcResult<ActorRef<KnowledgeActor>> {
    lookup::<KnowledgeActor>(KNOWLEDGE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "knowledge is disabled", None::<()>),
        _ => e,
    })
}

/// The codeindex actor; `codeindex.enabled: false` means it was never spawned (60 §10).
fn codeindex() -> RpcResult<ActorRef<CodeindexActor>> {
    lookup::<CodeindexActor>(CODEINDEX_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "codeindex is disabled", None::<()>),
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

/// Write `bytes` as `<data_dir>/attachments/<sha256[..16]>.<ext>`; an existing
/// file is not rewritten. Shared by `put_attachment` and the ACP prompt path;
/// the naming rule lives in `agent::context` (48 §3.3).
pub(crate) fn save_blob(bytes: &[u8], ext: &str, data_dir: &Path) -> std::io::Result<PathBuf> {
    agent::context::save_blob(bytes, ext, data_dir)
}

/// An `ASK_FAILED` (`-32000`) error with `msg`.
pub(crate) fn failed(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(ASK_FAILED, msg.into(), None::<()>)
}

/// Largest decoded image `put_attachment` accepts (17 §1: Anthropic's 5 MB); an
/// `.svg` (a saved diagram) shares the cap.
const MAX_IMAGE: usize = 5 * 1024 * 1024;
/// Largest decoded PDF `put_attachment` accepts (20 §2.5: Anthropic's 32 MB).
const MAX_PDF: usize = 32 * 1024 * 1024;
/// Largest decoded audio note `put_attachment` accepts (45 §7.1).
const MAX_AUDIO: usize = 16 * 1024 * 1024;

/// Validate `ext` and the size cap for its kind, then decode base64 `bytes`
/// (`put_attachment`, `put_session_attachment`).
fn decode_attachment(bytes: String, ext: &str) -> RpcResult<Vec<u8>> {
    let cap = match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => MAX_IMAGE,
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
async fn attach_pty(
    session: SessionId,
    job: &JobId,
    size: Option<(u16, u16)>,
) -> RpcResult<tools::PtyAttached> {
    lookup::<Jobs>(JOBS_ACTOR)?
        .ask(PtyAttach {
            session,
            id: job.clone(),
            size,
        })
        .await
        .map_err(|e| match e {
            SendError::HandlerError(msg) => failed(msg),
            other => failed(other.to_string()),
        })
}

#[async_trait]
impl ApiServer for Server {
    async fn info(&self) -> RpcResult<Info> {
        // 82 §3 / 83 §3: the supervisor's live defaults, not the config file read at startup.
        let default_model = ask::<Agents, _>(agent::AGENT_ACTOR, GetDefaultModel).await?;
        let default_effort = ask::<Agents, _>(agent::AGENT_ACTOR, GetDefaultEffort).await?;
        let favorite_models = ask::<Agents, _>(agent::AGENT_ACTOR, GetFavoriteModels).await?;
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
            _ => platform::process::hostname().to_owned(),
        };
        // 88: the schedule actor's live value; the config file when it is not running.
        let schedule_run_timeout_secs = match schedule() {
            Ok(s) => send(&s, schedule::GetRunTimeout).await?,
            Err(_) => self
                .rt
                .section::<schedule::ScheduleConfig>("schedule")
                .map(|c| c.run_timeout_secs)
                .unwrap_or(1800),
        };
        let communication =
            ask::<Agents, _>(agent::AGENT_ACTOR, GetCommunication { session: None }).await?;
        Ok(Info {
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            data_dir: self.rt.data_dir.display().to_string(),
            default_model,
            default_effort,
            favorite_models,
            approvals,
            compact_style,
            subagent_timeout_secs,
            schedule_run_timeout_secs,
            max_tool_rounds,
            stream_idle_secs,
            max_salvages,
            pty_idle_secs,
            name,
            communication,
            app_enabled: apps().is_ok(),
            // 125 §6.5: the supervisor's live advisor defaults (82 §3 rule).
            advisor: ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetAdvisorDefaults).await?,
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
        delivery: Option<Delivery>,
        id: Option<InputId>,
    ) -> RpcResult<SessionId> {
        self.authorize_opt(ext, session).await?;
        let session = match session {
            Some(s) => s,
            None => owner_root().await?,
        };
        if let Some(r) = forward::route(
            ext,
            session,
            "send_input",
            (Some(session), &text, forward::ByName(&files), &delivery, id),
        )
        .await?
        {
            return Ok(r);
        }
        let files = forward::resolve_files(ext, &self.rt.data_dir, files).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SendInput {
                session: Some(session),
                text,
                files,
                delivery: delivery.unwrap_or_default(),
                id,
            },
        )
        .await
        .map_err(|e| session_draining(session, e))
    }

    async fn steer(
        &self,
        ext: &Extensions,
        session: SessionId,
        text: String,
        id: Option<InputId>,
        files: Vec<PathBuf>,
        delivery: Option<Delivery>,
    ) -> RpcResult<InputId> {
        self.authorize(ext, session).await?;
        let id = id.unwrap_or_default();
        if let Some(r) = forward::route(
            ext,
            session,
            "steer",
            (session, &text, Some(id), forward::ByName(&files), &delivery),
        )
        .await?
        {
            return Ok(r);
        }
        let files = forward::resolve_files(ext, &self.rt.data_dir, files).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Steer {
                session,
                id,
                text,
                files,
                delivery: delivery.unwrap_or_default(),
            },
        )
        .await
        .map_err(|e| session_draining(session, e))?;
        Ok(id)
    }

    async fn peer_deliver(
        &self,
        ext: &Extensions,
        session: SessionId,
        envelope: PeerEnvelope,
    ) -> RpcResult<PeerReceipt> {
        self.authorize(ext, session).await?;
        if ext.get::<forward::Forwarded>().is_none() {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "peer_deliver is mesh-internal",
                None::<()>,
            ));
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, PeerDeliver { session, envelope }).await
    }

    async fn broadcast(
        &self,
        ext: &Extensions,
        req: BroadcastRequest,
    ) -> RpcResult<BroadcastReceipt> {
        // The supervisor owns the selector, so authorization is a two-step:
        // resolve as a dry run, drop what this device may not see (51 §4.2),
        // then send to the permitted ids by explicit list. The refused rows
        // are spliced back in selector order with their title emptied.
        let mut preview = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Broadcast(BroadcastRequest {
                dry_run: true,
                ..req.clone()
            }),
        )
        .await?;
        // Unknown ids (`Failed`) have nothing to authorize and stay as reported.
        let mut permitted = Vec::with_capacity(preview.rows.len());
        for row in &mut preview.rows {
            if matches!(row.outcome, BroadcastOutcome::Failed { .. }) {
                continue;
            }
            if self.authorize(ext, row.session).await.is_ok() {
                permitted.push(row.session);
            } else {
                row.title.clear();
                row.outcome = BroadcastOutcome::Skipped {
                    reason: "not visible from this device".into(),
                };
            }
        }
        if req.dry_run {
            return Ok(preview);
        }
        let allowed: HashSet<SessionId> = permitted.iter().copied().collect();
        let mut sent = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Broadcast(BroadcastRequest {
                targets: BroadcastTargets::Sessions { ids: permitted },
                ..req
            }),
        )
        .await?;
        // `Sessions` keeps the given order, so the sent rows zip with the
        // permitted preview rows.
        let mut rows = std::mem::take(&mut sent.rows).into_iter();
        sent.rows = preview
            .rows
            .into_iter()
            .map(|row| {
                if allowed.contains(&row.session) {
                    rows.next().unwrap_or(row)
                } else {
                    row
                }
            })
            .collect();
        Ok(sent)
    }

    async fn list_activity(&self, _ext: &Extensions) -> RpcResult<Vec<AgentInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListActivity).await
    }

    async fn withdraw(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: InputId,
    ) -> RpcResult<String> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "withdraw", (session, id)).await? {
            return Ok(r);
        }
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

    async fn cancel(
        &self,
        ext: &Extensions,
        session: SessionId,
        scope: Option<CancelScope>,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "cancel", (session, &scope)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Cancel {
                session,
                keep: None,
                scope: scope.unwrap_or_default(),
            },
        )
        .await
    }

    async fn list_jobs(&self, ext: &Extensions, session: SessionId) -> RpcResult<Vec<JobView>> {
        self.authorize(ext, session).await?;
        ask::<Jobs, _>(JOBS_ACTOR, ListJobs(session)).await
    }

    async fn list_live_counts(&self) -> RpcResult<Vec<LiveCountsRow>> {
        ask::<Jobs, _>(JOBS_ACTOR, LiveCounts).await
    }

    /// Every RPC caller is a human surface (TUI `/jobs kill`, the desktop
    /// row): the model learns of the kill by a one-line `<job_result>`.
    async fn cancel_job(&self, ext: &Extensions, session: SessionId, id: JobId) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Jobs, _>(
            JOBS_ACTOR,
            CancelJob {
                session,
                id,
                origin: CancelOrigin::Ui,
            },
        )
        .await
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
            .ask(PtyResize {
                session,
                id: job,
                rows,
                cols,
            })
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
        let item = |c: &PtyChunk| {
            SubscriptionMessage::from(
                serde_json::value::to_raw_value(c).expect("PtyChunk serializes"),
            )
        };
        tokio::spawn(async move {
            loop {
                let reset = PtyChunk::Reset {
                    data: b64(&at.replay),
                    rows: at.rows,
                    cols: at.cols,
                };
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

    async fn screen_context(
        &self,
        ext: &Extensions,
        req: computer::ScreenContextReq,
    ) -> RpcResult<computer::ScreenContextReply> {
        if peer(ext)?.remote() {
            return Err(failed("the screen is on the harness host"));
        }
        let actor = lookup::<Computer>(COMPUTER_ACTOR).map_err(|e| match e.code() {
            NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "disabled", None::<()>),
            _ => e,
        })?;
        send(&actor, ScreenContext(req)).await
    }

    async fn compact(
        &self,
        ext: &Extensions,
        session: SessionId,
        style: Option<CompactStyle>,
    ) -> RpcResult<u32> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "compact", (session, &style)).await? {
            return Ok(r);
        }
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
        if let Some(r) = forward::route(ext, session, "undo", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Undo { session }).await
    }

    async fn redo(&self, ext: &Extensions, session: SessionId) -> RpcResult<String> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "redo", (session,)).await? {
            return Ok(r);
        }
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

    async fn session_context_breakdown(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<ContextBreakdown> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SessionContextBreakdown { session }).await
    }

    // 137 §11.2 (137-contract.md §3): forwarded to the owning engine like
    // `compact`, else asked of the supervisor.
    async fn session_memory(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<SessionMemory> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "session_memory", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SessionMemoryQuery { session }).await
    }

    async fn endless_zoom(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: u64,
        n: u64,
    ) -> RpcResult<Vec<ViewLine>> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "endless_zoom", (session, id, n)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::EndlessZoom { session, id, n }).await
    }

    async fn endless_recall(
        &self,
        ext: &Extensions,
        session: SessionId,
        query: String,
        limit: Option<u8>,
    ) -> RpcResult<Vec<ViewLine>> {
        self.authorize(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "endless_recall", (session, &query, limit)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::EndlessRecall {
                session,
                query,
                limit,
            },
        )
        .await
    }

    async fn set_session_memory(
        &self,
        ext: &Extensions,
        session: SessionId,
        mode: MemoryMode,
    ) -> RpcResult<SessionMemory> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "set_session_memory", (session, mode)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::SetSessionMemory {
                session,
                mode,
                filter: agent::UnitFilter::All,
            },
        )
        .await
        .map(|r| r.memory)
    }

    async fn set_session_view_bytes(
        &self,
        ext: &Extensions,
        session: SessionId,
        bytes: u32,
    ) -> RpcResult<SessionMemory> {
        self.authorize(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "set_session_view_bytes", (session, bytes)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::SetSessionViewBytes { session, bytes },
        )
        .await
    }

    async fn remove_context(
        &self,
        ext: &Extensions,
        session: SessionId,
        expected_revision: String,
        targets: Vec<ContextTarget>,
    ) -> RpcResult<ContextRemoval> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "remove_context",
            (session, &expected_revision, &targets),
        )
        .await?
        {
            return Ok(r);
        }
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
        if let Some(r) = forward::route(ext, session, "regenerate", (session, turn)).await? {
            return Ok(r);
        }
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
        if let Some(r) = forward::route(
            ext,
            session,
            "edit_turn",
            (session, turn, &text, forward::ByName(&files)),
        )
        .await?
        {
            return Ok(r);
        }
        let files = forward::resolve_files(ext, &self.rt.data_dir, files).await?;
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

    async fn set_schedule_run_timeout(&self, seconds: u64) -> RpcResult<()> {
        send(&schedule()?, schedule::SetRunTimeout { seconds }).await
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

    async fn path_complete(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        input: String,
        cursor: usize,
    ) -> RpcResult<PathCompletion> {
        peer(ext)?;
        self.authorize_opt(ext, session).await?;
        let mut cwd = None;
        if let Some(session) = session {
            if let Some(owner) = forward::owner_peer(session).await {
                let forwarded: RpcResult<Option<PathCompletion>> = forward::to_peer(
                    ext,
                    &owner,
                    "path_complete",
                    (Some(session), &input, cursor),
                )
                .await;
                return Ok(forwarded.ok().flatten().unwrap_or_default());
            }
            let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
            cwd = store
                .session(session)
                .await
                .ok()
                .flatten()
                .and_then(|s| s.cwd)
                .map(PathBuf::from);
        }
        let cwd = cwd
            .or_else(|| std::env::current_dir().ok())
            .or_else(std::env::home_dir)
            .unwrap_or_default();
        Ok(crate::pathac::complete(&cwd, &input, cursor).await)
    }

    async fn session_cost(&self, ext: &Extensions, session: SessionId) -> RpcResult<SessionCost> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SessionCostQuery(session))
            .await?
            .ok_or_else(|| failed(format!("{session}: no accounting owner")))
    }

    async fn spend_report(&self, days: u32) -> RpcResult<SpendReport> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SpendReportQuery(days)).await
    }

    async fn session_take(
        &self,
        ext: &Extensions,
        session: SessionId,
        reason: String,
    ) -> RpcResult<SessionOwner> {
        self.authorize(ext, session).await?;
        let reason = sessionsync::TakeReason::parse(&reason).ok_or_else(|| {
            ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                format!("reason {reason:?}: expected user | orphan | write"),
                None::<()>,
            )
        })?;
        let (owner, epoch) = ask::<sessionsync::SessionSync, _>(
            sessionsync::SESSIONS_ACTOR,
            sessionsync::TakeSession {
                id: session,
                reason,
                // The explicit `reason` is the caller's intent (`user` resumes).
                by_user: false,
            },
        )
        .await?;
        Ok(SessionOwner {
            owner,
            epoch,
            here: true,
        })
    }

    async fn session_owner(&self, ext: &Extensions, session: SessionId) -> RpcResult<SessionOwner> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        sessionsync::owner(&store, session)
            .await
            .map_err(|e| failed(format!("{e:#}")))?
            .ok_or_else(|| failed(format!("{session}: no such session")))
    }

    async fn session_orphans(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<SessionOrphan>> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        sessionsync::orphans(&store, session)
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn sessions_status(&self) -> RpcResult<SessionsStatus> {
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let sessions = ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions).await?;
        let here = sessions.iter().filter(|s| s.here).count();
        Ok(SessionsStatus {
            engine: store.engine().to_owned(),
            here: u32::try_from(here).unwrap_or(u32::MAX),
            elsewhere: u32::try_from(sessions.len() - here).unwrap_or(u32::MAX),
            log_seq: u64::try_from(store.log_seq()).unwrap_or_default(),
            // ponytail: worker L owns snapshots; surface its timestamp here once it records one.
            last_snapshot: None,
        })
    }

    async fn rename_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        title: String,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "rename_session", (session, &title)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, RenameSession { session, title }).await
    }

    async fn pin_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        pinned: bool,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "pin_session", (session, pinned)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, PinSession { session, pinned }).await
    }

    async fn archive_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        archived: bool,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "archive_session", (session, archived)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, ArchiveSession { session, archived }).await
    }

    async fn delete_session(&self, ext: &Extensions, session: SessionId) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "delete_session", (session,)).await? {
            return Ok(r);
        }
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
        if let Some(r) =
            forward::route(ext, session, "set_session_project", (session, &project)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, SetSessionProject { session, project }).await
    }

    async fn list_project_styles(&self) -> RpcResult<Vec<ProjectStyle>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListProjectStyles).await
    }

    async fn set_project_style(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
    ) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetProjectStyle { name, icon, color }).await
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
        tracing::debug!(%query, ?categories, ?limit, "rpc search");
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

    async fn resolve_approval(
        &self,
        ext: &Extensions,
        id: ApprovalId,
        decision: Decision,
    ) -> RpcResult<bool> {
        if let Some(r) = route_question(ext, &id.0, "resolve_approval", (id, decision)).await? {
            return Ok(r);
        }
        let pending = ask::<Approval, _>(tools::APPROVAL_ACTOR, Resolve { id, decision }).await?;
        // 130 E4: the receipt the companion reads back (`activity_list`).
        if pending {
            let what = match decision {
                Decision::Allow => "allowed",
                Decision::Deny => "denied",
            };
            tasks::activity::record(
                &self.rt,
                tasks::activity::ActivityKind::ApprovalResolved,
                &crate::Client::actor(ext),
                &id.0.to_string(),
                format!("{what} approval {}", id.0),
            )
            .await;
        }
        Ok(pending)
    }

    async fn answer_ask(
        &self,
        ext: &Extensions,
        id: AskId,
        answers: Option<Vec<Answer>>,
    ) -> RpcResult<bool> {
        if let Some(r) = route_question(ext, &id.0, "answer_ask", (id, &answers)).await? {
            return Ok(r);
        }
        let what = if answers.is_some() {
            "answered"
        } else {
            "dismissed"
        };
        let pending = agent::answer_ask(id, answers)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>))?;
        if pending {
            tasks::activity::record(
                &self.rt,
                tasks::activity::ActivityKind::AskAnswered,
                &crate::Client::actor(ext),
                &id.0.to_string(),
                format!("{what} question {}", id.0),
            )
            .await;
        }
        Ok(pending)
    }

    async fn answer_page_preview(
        &self,
        id: artifact::preview::PreviewId,
        answer: artifact::preview::PagePreviewAnswer,
    ) -> RpcResult<bool> {
        Ok(artifact::preview::answer(id, answer))
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
        manual: Option<bool>,
    ) -> RpcResult<ModelInfo> {
        self.authorize_opt(ext, session).await?;
        let manual = manual.unwrap_or(true);
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "set_model", (session, &model, manual)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetModel {
                session,
                model,
                manual,
            },
        )
        .await
    }

    async fn set_model_favorite(&self, model: String, favorite: bool) -> RpcResult<Vec<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetModelFavorite { model, favorite }).await
    }

    async fn set_effort(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        effort: Option<String>,
        manual: Option<bool>,
    ) -> RpcResult<()> {
        self.authorize_opt(ext, session).await?;
        let manual = manual.unwrap_or(true);
        if let Some(s) = session
            && let Some(r) =
                forward::route(ext, s, "set_effort", (session, &effort, manual)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetEffort {
                session,
                effort,
                manual,
            },
        )
        .await
    }

    async fn set_advisor(
        &self,
        ext: &Extensions,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
    ) -> RpcResult<AdvisorStatus> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisor {
                session,
                model,
                effort,
            },
        )
        .await
    }

    async fn advisor_status(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<AdvisorStatus> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, AdvisorStatusOf { session }).await
    }

    async fn set_advisor_default_effort(
        &self,
        effort: Option<String>,
    ) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                effort: Some(effort),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn set_advisor_notes_per_update(&self, n: u8) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                max_notes_per_update: Some(n),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn set_advisor_immune_turns(&self, n: u8) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                immune_turns: Some(n),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn set_advisor_sync_backlog(&self, v: SyncBacklog) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                sync_backlog: Some(v),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn harness_options(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<acp::ConfigOption>> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "harness_options", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, HarnessOptions { session }).await
    }

    async fn set_harness_option(
        &self,
        ext: &Extensions,
        session: SessionId,
        config_id: String,
        value: String,
    ) -> RpcResult<acp::SetOptionResult> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "set_harness_option",
            (session, &config_id, &value),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetHarnessOption {
                session,
                config_id,
                value,
            },
        )
        .await
    }

    async fn resume_child(&self, ext: &Extensions, session: SessionId) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "resume_child", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, ResumeChild { session }).await
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
        if let Some(r) =
            forward::route(ext, session, "set_mcp_hidden", (session, &tools, hidden)).await?
        {
            return Ok(r);
        }
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
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "set_approvals", (session, enabled)).await?
        {
            return Ok(r);
        }
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
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "set_communication", (session, enabled)).await?
        {
            return Ok(r);
        }
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
        if let Some(r) = forward::route(ext, root, "set_peer_scope", (root, &scope, &alias)).await?
        {
            return Ok(r);
        }
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
                active_only: false,
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
        if let Some(r) = forward::route(
            ext,
            session,
            "answer_peer_request",
            (session, reply_to, &text),
        )
        .await?
        {
            return Ok(r);
        }
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

    async fn send_message(
        &self,
        ext: &Extensions,
        session: SessionId,
        to: String,
        text: String,
    ) -> RpcResult<PeerReceipt> {
        self.authorize(ext, session).await?;
        let d = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::supervisor::Deliver {
                from: session,
                to,
                text,
                await_reply: false,
                reply_to: None,
                request_id: None,
                admitted: tokio_util::sync::CancellationToken::new(),
            },
        )
        .await?;
        Ok(d.receipt)
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
        if let Some(r) =
            forward::route(ext, session, "use_skill", (Some(session), &name, &args)).await?
        {
            return Ok(r);
        }
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

    async fn doc_list(
        &self,
        kind: Option<document::DocKind>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<document::DocSummary>> {
        doc_send(&documents()?, document::ListDocs { kind, limit }).await
    }

    async fn doc_resolve(
        &self,
        ext: &Extensions,
        binding: document::Binding,
    ) -> RpcResult<Option<document::DocId>> {
        let docs = documents()?;
        if let Some(id) = doc_send(
            &docs,
            document::Lookup {
                binding: binding.clone(),
            },
        )
        .await?
        {
            return Ok(Some(id));
        }
        let text = match &binding {
            // On demand (59 §7.1): a knowledge concept becomes a document from its current file.
            document::Binding::Knowledge(concept) => match send(
                &knowledge()?,
                Get {
                    id: concept.clone(),
                },
            )
            .await
            {
                Ok(c) => c.text,
                Err(e) if e.code() == ASK_FAILED => return Ok(None),
                Err(e) => return Err(e),
            },
            // An agent's charter becomes a document from the body of `agents/<name>.md`; theuth's is
            // `prompt.md`, read-only, never a document.
            document::Binding::AgentCharter(name) if name == roster::AgentName::DEFAULT => {
                return Ok(None);
            }
            document::Binding::AgentCharter(name) => {
                match send(&roster_actor()?, roster::GetAgent(name.clone())).await {
                    Ok(f) => f.body,
                    Err(e) if e.code() == ASK_FAILED => return Ok(None),
                    Err(e) => return Err(e),
                }
            }
            _ => return Ok(None),
        };
        let r = doc_send(
            &docs,
            document::Ensure {
                binding: binding.clone(),
                markdown: text,
            },
        )
        .await
        .map(Some);
        doc_forward(ext, "doc_resolve", (&binding,), r).await
    }

    async fn doc_ensure(
        &self,
        ext: &Extensions,
        binding: document::Binding,
        markdown: String,
    ) -> RpcResult<document::DocId> {
        if !matches!(&binding, document::Binding::AgentCharter(name) if name != roster::AgentName::DEFAULT)
        {
            return Err(ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "owner_creates",
                None::<()>,
            ));
        }
        let r = doc_send(
            &documents()?,
            document::Ensure {
                binding: binding.clone(),
                markdown: markdown.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_ensure", (&binding, &markdown), r).await
    }

    async fn doc_markdown(&self, doc: document::DocId) -> RpcResult<String> {
        doc_send(
            &documents()?,
            document::Markdown {
                doc,
                blocks: None,
                annotate: false,
            },
        )
        .await
    }

    async fn doc_export(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        format: artifact::ExportFormat,
        dest: Option<PathBuf>,
        assets: Vec<artifact::ExportAsset>,
    ) -> RpcResult<artifact::ExportResult> {
        let (peer, by) = principal(ext)?;
        if let Some(dest) = &dest {
            owner(ext)?;
            let home = dirs::home_dir().ok_or_else(|| failed("no home directory"))?;
            let inside = dest.is_absolute()
                && dest.starts_with(&home)
                && !dest
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir));
            if !inside {
                return Err(failed(format!(
                    "{}: not under {}",
                    dest.display(),
                    home.display()
                )));
            }
        }
        let docs = documents()?;
        let meta = doc_send(
            &docs,
            document::MetaOf {
                doc: doc.clone(),
                by,
            },
        )
        .await;
        member_outcome(&peer, &meta);
        let meta = meta?;
        let annotate = format == artifact::ExportFormat::Pdf;
        let markdown = doc_send(
            &docs,
            document::Markdown {
                doc,
                blocks: None,
                annotate,
            },
        )
        .await?;
        send(
            &artifact()?,
            artifact::ExportDoc {
                title: meta.title,
                format,
                markdown,
                dest,
                assets,
                record: None,
            },
        )
        .await
    }

    async fn doc_open(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        sv: Option<String>,
    ) -> RpcResult<document::DocOpen> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(&documents()?, document::OpenDoc { doc, sv, by }).await;
        member_outcome(&peer, &r);
        r
    }

    async fn doc_update(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        update: String,
    ) -> RpcResult<()> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(&documents()?, document::ApplyUpdate { doc, update, by }).await;
        member_outcome(&peer, &r);
        r
    }

    async fn doc_awareness(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        state: String,
    ) -> RpcResult<()> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(
            &documents()?,
            document::AwarenessUpdateMsg { doc, state, by },
        )
        .await;
        member_outcome(&peer, &r);
        r
    }

    async fn doc_members(
        &self,
        ext: &Extensions,
        doc: document::DocId,
    ) -> RpcResult<document::Members> {
        owner(ext)?;
        let r = doc_send(&documents()?, document::MembersOf { doc: doc.clone() }).await;
        doc_forward(ext, "doc_members", (&doc,), r).await
    }

    async fn doc_member_set(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        member: String,
        role: Option<document::Role>,
        name: Option<String>,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::MemberSet {
                doc: doc.clone(),
                pub_: member.clone(),
                role,
                name: name.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_member_set", (&doc, &member, role, &name), r).await
    }

    async fn doc_member_revoke(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        member: String,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::MemberRevoke {
                doc: doc.clone(),
                pub_: member.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_member_revoke", (&doc, &member), r).await?;
        // 59 §8.4: their live conns go too — here as well as on the hub (the guest may hang off either
        // engine's uplink). Link off (no conns) is not an error.
        if let Ok(link) = link()
            && let Ok(key) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&member)
            && let Ok(key) = <[u8; 32]>::try_from(key)
        {
            let _ = link.ask(link::DropGuest { key }).await;
        }
        Ok(())
    }

    async fn doc_link_create(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        role: document::Role,
        name: Option<String>,
        expires_ms: Option<i64>,
    ) -> RpcResult<document::LinkCreated> {
        owner(ext)?;
        // The URL needs the relay host and an id devices dial (59 §8.2): Link must be on. 69: the mesh
        // device id when this engine has one — the hub, whichever engine it is, answers `/h/<mesh id>`
        // — else the engine id (a single engine before any device paired). Old engine-id links keep
        // resolving: the engine uplink stays registered.
        let status: LinkStatus = send(&link()?, link::Status).await?;
        if !status.enabled {
            return Err(failed(
                "link is off: turn on Settings › Link to share documents",
            ));
        }
        let harness = status
            .mesh
            .as_ref()
            .map(|m| m.device_id.clone())
            .or(status.harness_id)
            .ok_or_else(|| failed("link has no identity yet"))?;
        let relay = status.relay.trim_end_matches('/');
        let origin = relay
            .strip_prefix("wss://")
            .map(|h| format!("https://{h}"))
            .or_else(|| relay.strip_prefix("ws://").map(|h| format!("http://{h}")))
            .unwrap_or_else(|| relay.to_owned());
        let r = doc_send(
            &documents()?,
            document::LinkCreate {
                doc: doc.clone(),
                role,
                name: name.clone(),
                expires_ms,
            },
        )
        .await
        .map(|(link, secret)| document::LinkCreated {
            url: format!("{origin}/d#1.{harness}.{doc}.{secret}"),
            link,
        });
        doc_forward(ext, "doc_link_create", (&doc, role, &name, expires_ms), r).await
    }

    async fn doc_link_revoke(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        link: String,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::LinkRevoke {
                doc: doc.clone(),
                link: link.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_link_revoke", (&doc, &link), r).await
    }

    async fn doc_join(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        proof: String,
        name: Option<String>,
    ) -> RpcResult<document::Joined> {
        let peer = peer(ext)?;
        let Some(guest) = peer.guest() else {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "guests only",
                None::<()>,
            ));
        };
        let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(proof)
            .map_err(|_| failed("bad_proof"))?;
        let r = doc_send(
            &documents()?,
            document::Join {
                doc,
                guest_pub: guest.key,
                handshake_hash: guest.handshake_hash.to_vec(),
                proof,
                name,
            },
        )
        .await;
        match &r {
            Ok(_) => guest.mark_joined(),
            Err(_) => {
                guest.refuse();
            }
        }
        r
    }

    async fn doc_set_name(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        name: String,
    ) -> RpcResult<()> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(&documents()?, document::SetName { doc, by, name }).await;
        member_outcome(&peer, &r);
        r
    }

    async fn notebook_list(&self, archived: Option<bool>) -> RpcResult<Vec<document::Notebook>> {
        let mut list = doc_send(
            &documents()?,
            document::NotebookList {
                archived: archived.unwrap_or(false),
            },
        )
        .await?;
        let counts = notebook_chat_counts().await;
        for nb in &mut list {
            nb.chats = counts.get(&nb.doc.id.to_string()).copied().unwrap_or(0);
        }
        Ok(list)
    }

    async fn notebook_get(
        &self,
        ext: &Extensions,
        id: document::DocId,
    ) -> RpcResult<document::NotebookDetail> {
        let (peer, _by) = principal(ext)?;
        let mut r = doc_send(&documents()?, document::NotebookGet { id }).await;
        member_outcome(&peer, &r);
        if let Ok(d) = &mut r {
            d.notebook.chats = notebook_chat_counts()
                .await
                .get(&d.notebook.doc.id.to_string())
                .copied()
                .unwrap_or(0);
        }
        r
    }

    async fn notebook_create(
        &self,
        ext: &Extensions,
        title: String,
        description: Option<String>,
        project: Option<String>,
    ) -> RpcResult<document::Notebook> {
        owner(ext)?;
        if title.trim().is_empty() {
            return Err(ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "title must not be empty",
                None::<()>,
            ));
        }
        let r = doc_send(
            &documents()?,
            document::NotebookCreate {
                title: title.trim().to_owned(),
                description: description.clone().unwrap_or_default(),
                project: project.clone().filter(|p| !p.trim().is_empty()),
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "notebook_create", (&title, &description, &project), r).await
    }

    async fn notebook_update(
        &self,
        ext: &Extensions,
        id: document::DocId,
        title: Option<String>,
        project: Option<String>,
        archived: Option<bool>,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::NotebookUpdate {
                id: id.clone(),
                title: title.clone().filter(|t| !t.trim().is_empty()),
                project: project
                    .clone()
                    .map(|p| Some(p).filter(|p| !p.trim().is_empty())),
                archived,
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(
            ext,
            "notebook_update",
            (&id, &title, &project, &archived),
            r,
        )
        .await
    }

    async fn notebook_delete_preview(
        &self,
        id: document::DocId,
    ) -> RpcResult<document::DeletePreview> {
        doc_send(&documents()?, document::NotebookDeletePreview { id }).await
    }

    async fn notebook_delete(
        &self,
        ext: &Extensions,
        id: document::DocId,
        delete_exclusive_sources: bool,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::NotebookDelete {
                id: id.clone(),
                delete_exclusive_sources,
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "notebook_delete", (&id, &delete_exclusive_sources), r).await
    }

    async fn notebook_items_list(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
    ) -> RpcResult<Vec<document::NotebookItem>> {
        let (peer, _by) = principal(ext)?;
        let r = doc_send(&documents()?, document::NotebookItemsList { notebook }).await;
        member_outcome(&peer, &r);
        r
    }

    async fn notebook_item_add(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
        kind: document::ItemKind,
        item: String,
    ) -> RpcResult<document::NotebookItem> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::NotebookItemAdd {
                notebook: notebook.clone(),
                kind,
                item: item.clone(),
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "notebook_item_add", (&notebook, &kind, &item), r).await
    }

    async fn notebook_item_remove(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
        kind: document::ItemKind,
        item: String,
    ) -> RpcResult<document::ItemRemoved> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::NotebookItemRemove {
                notebook: notebook.clone(),
                kind,
                item: item.clone(),
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "notebook_item_remove", (&notebook, &kind, &item), r).await
    }

    async fn notebook_chats(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
    ) -> RpcResult<Vec<SessionSummary>> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::notebooks::notebook_chats(&agents, notebook.to_string()).await)
    }

    async fn session_set_notebook(
        &self,
        ext: &Extensions,
        session: SessionId,
        notebook: Option<document::DocId>,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::notebooks::session_set_notebook(
                &agents,
                session,
                notebook.map(|n| n.to_string()),
            )
            .await,
        )
    }

    async fn source_add(
        &self,
        ext: &Extensions,
        args: document::ingest::SourceAddArgs,
    ) -> RpcResult<document::Source> {
        owner(ext)?;
        let r = nb_result(
            document::ingest::source_add(
                &documents()?,
                args.clone(),
                document::Principal::Owner { device: None },
            )
            .await,
        );
        doc_forward(ext, "source_add", (&args,), r).await
    }

    async fn source_get(
        &self,
        ext: &Extensions,
        id: document::DocId,
    ) -> RpcResult<document::Source> {
        let (peer, _by) = principal(ext)?;
        let r = nb_result(document::ingest::source_get(&documents()?, id).await);
        member_outcome(&peer, &r);
        r
    }

    async fn source_list(
        &self,
        notebook: Option<document::DocId>,
    ) -> RpcResult<Vec<document::Source>> {
        nb_result(document::ingest::source_list(&documents()?, notebook).await)
    }

    async fn source_refresh(
        &self,
        ext: &Extensions,
        id: document::DocId,
    ) -> RpcResult<document::Source> {
        owner(ext)?;
        let r = nb_result(
            document::ingest::source_refresh(
                &documents()?,
                id.clone(),
                document::Principal::Owner { device: None },
            )
            .await,
        );
        doc_forward(ext, "source_refresh", (&id,), r).await
    }

    async fn cite_resolve(
        &self,
        ext: &Extensions,
        hrefs: Vec<String>,
    ) -> RpcResult<Vec<document::Citation>> {
        let (peer, by) = principal(ext)?;
        let mut r = doc_send(&documents()?, document::CiteResolve { hrefs, by }).await;
        member_outcome(&peer, &r);
        if let Ok(cites) = &mut r {
            check_session_cites(cites).await;
        }
        r
    }

    async fn nb_search(
        &self,
        args: document::index::NbSearchArgs,
    ) -> RpcResult<Vec<document::index::BlockHit>> {
        nb_result(document::index::nb_search(&documents()?, args).await)
    }

    async fn save_answer_as_note(
        &self,
        ext: &Extensions,
        session: SessionId,
        turn: u32,
        notebook: document::DocId,
    ) -> RpcResult<agent::notebooks::SavedAnswer> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::notebooks::save_answer_as_note(&agents, session, turn, notebook.to_string())
                .await,
        )
    }

    async fn promote_answer(
        &self,
        ext: &Extensions,
        session: SessionId,
        turn: u32,
        note: document::DocId,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::notebooks::promote_answer(&agents, session, turn, note.to_string()).await)
    }

    async fn doc_snapshots(&self, doc: document::DocId) -> RpcResult<Vec<document::SnapshotInfo>> {
        doc_send(&documents()?, document::Snapshots { doc }).await
    }

    async fn doc_restore(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        snapshot: String,
    ) -> RpcResult<()> {
        let r = doc_send(
            &documents()?,
            document::Restore {
                doc: doc.clone(),
                snapshot: snapshot.clone(),
            },
        )
        .await
        .map(|_| ());
        doc_forward(ext, "doc_restore", (&doc, &snapshot), r).await
    }

    async fn doc_undo_agent(&self, ext: &Extensions, doc: document::DocId) -> RpcResult<()> {
        let r = doc_send(
            &documents()?,
            document::UndoLastAgentEdit { doc: doc.clone() },
        )
        .await
        .map(|_| ());
        doc_forward(ext, "doc_undo_agent", (&doc,), r).await
    }

    async fn doc_create(
        &self,
        ext: &Extensions,
        title: String,
        markdown: Option<String>,
    ) -> RpcResult<document::DocSummary> {
        if title.trim().is_empty() {
            return Err(ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "title must not be empty",
                None::<()>,
            ));
        }
        let r = doc_send(
            &documents()?,
            document::CreateNote {
                title: title.trim().to_owned(),
                markdown: markdown.clone().unwrap_or_default(),
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "doc_create", (&title, &markdown), r).await
    }

    async fn documents_status(&self) -> RpcResult<document::HelperStatus> {
        Ok(document::helper_status())
    }

    async fn doc_stats(&self) -> RpcResult<document::DocStats> {
        doc_send(&documents()?, document::Stats).await
    }

    async fn doc_rename(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        title: String,
    ) -> RpcResult<()> {
        let r = doc_send(
            &documents()?,
            document::Rename {
                doc: doc.clone(),
                title: title.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_rename", (&doc, &title), r).await
    }

    async fn doc_delete(&self, ext: &Extensions, doc: document::DocId) -> RpcResult<()> {
        let r = doc_send(&documents()?, document::DeleteNote { doc: doc.clone() }).await;
        doc_forward(ext, "doc_delete", (&doc,), r).await
    }

    async fn knowledge_harvest(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<HarvestResult> {
        self.authorize(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "knowledge_harvest", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Harvest { session }).await
    }

    async fn codeindex_stats(&self, root: Option<String>) -> RpcResult<IndexStats> {
        send(
            &codeindex()?,
            codeindex::Stats {
                root: root.map(PathBuf::from),
            },
        )
        .await
    }

    async fn code_locate(
        &self,
        root: String,
        anchors: Vec<LocateAnchor>,
    ) -> RpcResult<Vec<Located>> {
        send(
            &codeindex()?,
            codeindex::Locate {
                root: PathBuf::from(root),
                anchors,
            },
        )
        .await
    }

    async fn codemap_here(
        &self,
        root: String,
        focus: Option<Vec<String>>,
        edited: Option<Vec<String>>,
        limit: Option<u32>,
    ) -> RpcResult<HereLens> {
        send(
            &codeindex()?,
            codeindex::Here {
                root: PathBuf::from(root),
                focus,
                edited,
                limit,
            },
        )
        .await
    }

    async fn codemap_fit(
        &self,
        root: String,
        symbol: String,
        limit: Option<u32>,
    ) -> RpcResult<FitLens> {
        send(
            &codeindex()?,
            codeindex::Fit {
                root: PathBuf::from(root),
                symbol,
                limit,
            },
        )
        .await
    }

    async fn codemap_change(
        &self,
        root: String,
        git_range: Option<String>,
        files: Option<Vec<String>>,
        include_tests: Option<bool>,
    ) -> RpcResult<ChangeLens> {
        send(
            &codeindex()?,
            codeindex::Change {
                root: PathBuf::from(root),
                git_range,
                files,
                include_tests,
            },
        )
        .await
    }

    async fn codemap_touch(&self, root: String, paths: Option<Vec<String>>) -> RpcResult<LensMeta> {
        send(
            &codeindex()?,
            codeindex::Touch {
                root: PathBuf::from(root),
                paths,
            },
        )
        .await
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

    async fn telegram_status(&self) -> RpcResult<TelegramStatus> {
        send(&telegram()?, telegram::Status).await
    }

    async fn telegram_link(&self, owner: Option<u64>) -> RpcResult<Option<PairingLink>> {
        send(&telegram()?, telegram::Link { owner }).await
    }

    async fn telegram_allow(&self, user: u64) -> RpcResult<()> {
        send(&telegram()?, telegram::Allow(user)).await
    }

    async fn telegram_remove_allowed(&self, user: u64) -> RpcResult<()> {
        send(&telegram()?, telegram::RemoveAllowed(user)).await
    }

    async fn telegram_allow_group(&self, chat: i64) -> RpcResult<()> {
        send(&telegram()?, telegram::AllowGroup(chat)).await
    }

    async fn telegram_remove_group(&self, chat: i64) -> RpcResult<()> {
        send(&telegram()?, telegram::RemoveGroup(chat)).await
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

    async fn shutdown(&self, ext: &Extensions) -> RpcResult<()> {
        match ext.get::<link::RpcPeer>() {
            Some(p) => tracing::warn!(
                peer = p.id(),
                remote = p.remote(),
                "shutdown requested over rpc"
            ),
            None => tracing::warn!("shutdown requested over rpc (unauthenticated connection)"),
        }
        // 109 §9.2: the drain runs before the token (every turn's cancel is
        // its child); off the request so the caller's reply is immediate.
        let rt = self.rt.clone();
        tokio::spawn(async move {
            agent::supervisor::drain_engine(&rt, "shutdown").await;
            rt.shutdown.cancel();
        });
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

    async fn devices_list(&self) -> RpcResult<Vec<link::DeviceInfo>> {
        send(&link()?, link::DevicesList).await
    }

    async fn device_revoke(&self, id: String) -> RpcResult<()> {
        send(&link()?, Revoke { id }).await
    }

    async fn device_rename(&self, id: String, name: String) -> RpcResult<()> {
        send(&link()?, RenameDevice { id, name }).await
    }

    async fn device_pair_start(&self, name: Option<String>) -> RpcResult<link::PairingPayload> {
        send(&link()?, link::DevicePairStart { name }).await
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

    async fn list_sensors(&self) -> RpcResult<Vec<schedule::SensorInfo>> {
        send(&schedule()?, schedule::ListSensors).await
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
        // 43's button: a replacing utterance the client plays (76 §0.3 keeps this exactly).
        send(
            &voice()?,
            voice::Speak {
                session,
                text,
                queue: false,
                sink: voice::Sink::Client,
            },
        )
        .await
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
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
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

    async fn talk_start(
        &self,
        ext: &Extensions,
        session: SessionId,
        sink: talk::Sink,
        sample_rate: Option<u32>,
    ) -> RpcResult<talk::TalkId> {
        let peer = peer(ext)?;
        match (sink, sample_rate) {
            (talk::Sink::Device, _) if peer.remote() => {
                return Err(failed(
                    "speaker and microphone are on the harness host; pass sink: client and sample_rate to stream audio",
                ));
            }
            (talk::Sink::Client, None) => {
                return Err(failed("sink: client needs sample_rate"));
            }
            _ => {}
        }
        let actor = talk()?;
        let id = send(
            &actor,
            talk::StartTalk {
                session,
                sink,
                sample_rate,
            },
        )
        .await?;
        // The talk is bound to the connection that started it (76 §0.10): a
        // dropped connection must not hold the mic, the speaker or the slot.
        tokio::spawn(async move {
            peer.closed().await;
            let _ = send(&actor, talk::StopTalk { id }).await;
        });
        Ok(id)
    }

    async fn talk_stop(&self, ext: &Extensions, id: talk::TalkId) -> RpcResult<()> {
        peer(ext)?;
        send(&talk()?, talk::StopTalk { id }).await
    }

    async fn talk_audio(
        &self,
        ext: &Extensions,
        id: talk::TalkId,
        pcm: String,
        played_seq: Option<u32>,
    ) -> RpcResult<()> {
        peer(ext)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(pcm)
            .map_err(|e| failed(format!("bad base64: {e}")))?;
        if bytes.len() % 2 != 0 {
            return Err(failed("pcm must be little-endian i16"));
        }
        let pcm = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        send(
            &talk()?,
            talk::PushAudio {
                id,
                pcm,
                played_seq,
            },
        )
        .await
    }

    async fn talk_status(&self, ext: &Extensions) -> RpcResult<talk::TalkState> {
        peer(ext)?;
        send(&talk()?, talk::TalkStatus).await
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

    async fn decision_mode(&self, prefix: String) -> RpcResult<decision::ModeInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::GetMode { prefix }).await
    }

    async fn set_decision_mode(
        &self,
        prefix: String,
        mode: decision::Mode,
    ) -> RpcResult<decision::ModeInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::SetMode { prefix, mode })
            .await
    }

    // 126: the read / ladder methods ask the double actor (W1); `double_train`, `double_rollback`,
    // `double_export`, `double_delete_all` and `double_settings_set` ask the scheduler (W2). With
    // `double.enabled: false` neither actor exists and `lookup` answers `NOT_RUNNING`, never a panic.

    async fn double_status(&self) -> RpcResult<double::DoubleStatus> {
        match ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::Status).await {
            Ok(s) => Ok(s),
            // Disabled: the plugin registered nothing; report the section as read.
            Err(e) if e.code() == NOT_RUNNING => {
                let cfg = self.double_config();
                Ok(double::DoubleStatus {
                    enabled: cfg.enabled,
                    venue: cfg.training.venue,
                    ..double::DoubleStatus::default()
                })
            }
            Err(e) => Err(e),
        }
    }

    async fn double_classes(&self) -> RpcResult<Vec<double::DoubleClass>> {
        ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::Classes).await
    }

    async fn double_class(&self, key: String) -> RpcResult<double::DoubleClassDetail> {
        ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::ClassDetail(key)).await
    }

    async fn double_train(
        &self,
        tier: double::Tier,
        venue: double::Venue,
    ) -> RpcResult<double::RunId> {
        ask::<double::sched::Scheduler, _>(
            double::sched::ACTOR,
            double::sched::Train { tier, venue },
        )
        .await
    }

    async fn double_pause(&self, on: bool) -> RpcResult<double::DoubleStatus> {
        ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::Pause(on)).await
    }

    async fn double_forget(&self, target: double::ForgetTarget) -> RpcResult<u32> {
        ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::Forget(target)).await
    }

    async fn double_rollback(&self, version: String) -> RpcResult<double::DoubleStatus> {
        ask::<double::sched::Scheduler, _>(double::sched::ACTOR, double::sched::Rollback(version))
            .await
    }

    async fn double_set_cap(
        &self,
        class: String,
        rung: u8,
        mutating_opt_in: bool,
    ) -> RpcResult<double::DoubleClass> {
        ask::<double::actor::Double, _>(
            double::DOUBLE_ACTOR,
            double::actor::SetCap {
                class,
                rung,
                mutating_opt_in,
            },
        )
        .await
    }

    async fn double_export(&self) -> RpcResult<double::DoubleExport> {
        ask::<double::sched::Scheduler, _>(double::sched::ACTOR, double::sched::Export).await
    }

    async fn double_delete_all(&self) -> RpcResult<()> {
        ask::<double::sched::Scheduler, _>(double::sched::ACTOR, double::sched::DeleteAll).await
    }

    async fn double_take_over(&self, question_id: String) -> RpcResult<bool> {
        match ask::<double::actor::Double, _>(
            double::DOUBLE_ACTOR,
            double::actor::TakeOver(question_id),
        )
        .await
        {
            Ok(b) => Ok(b),
            // No actor: no grace can be running; nothing to cancel.
            Err(e) if e.code() == NOT_RUNNING => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn double_settings_get(&self) -> RpcResult<double::DoubleConfig> {
        Ok(self.double_config())
    }

    async fn double_settings_set(
        &self,
        settings: double::DoubleConfig,
    ) -> RpcResult<double::DoubleConfig> {
        ask::<double::sched::Scheduler, _>(
            double::sched::ACTOR,
            double::sched::SetSettings(settings),
        )
        .await
    }

    // 136 §2.7: every method asks the `dreaming` actor through `dreaming::rpc`; with
    // `dreaming.enabled: false` the actor does not exist — `status` and `settings` then answer from
    // the section and `state.json`, the rest report `NOT_RUNNING`.

    async fn dreaming_status(&self) -> RpcResult<dreaming::DreamStatus> {
        match dreaming::rpc::status().await {
            Ok(s) => Ok(s),
            Err(_) => Ok(dreaming::night::status(
                &self.rt.data_dir,
                &self.dreaming_config(),
            )),
        }
    }

    async fn dreaming_nights(&self, limit: u32) -> RpcResult<Vec<dreaming::NightSummary>> {
        dreaming::rpc::nights(limit).await.map_err(dreaming_err)
    }

    async fn dreaming_run_now(&self, request: dreaming::RunRequest) -> RpcResult<SessionId> {
        dreaming::rpc::run_now(request).await.map_err(dreaming_err)
    }

    async fn dreaming_undo(
        &self,
        night: String,
        group: Option<String>,
    ) -> RpcResult<dreaming::UndoReport> {
        dreaming::rpc::undo(night, group)
            .await
            .map_err(dreaming_err)
    }

    async fn dreaming_settings(&self) -> RpcResult<dreaming::DreamSettings> {
        match dreaming::rpc::settings().await {
            Ok(s) => Ok(s),
            Err(_) => Ok(dreaming::night::settings(
                &self.rt.data_dir,
                &self.dreaming_config(),
                &dreaming::config::AgentDefaults::from_runtime(&self.rt),
            )),
        }
    }

    async fn dreaming_set_settings(
        &self,
        settings: dreaming::DreamSettings,
    ) -> RpcResult<dreaming::DreamSettings> {
        dreaming::rpc::set_settings(settings)
            .await
            .map_err(dreaming_err)
    }

    async fn dreaming_proposals(&self) -> RpcResult<Vec<dreaming::Proposal>> {
        dreaming::rpc::proposals().await.map_err(dreaming_err)
    }

    async fn dreaming_proposal_act(&self, id: String, choice: String) -> RpcResult<()> {
        dreaming::rpc::proposal_act(id, choice)
            .await
            .map_err(dreaming_err)
    }

    async fn dreaming_metrics(&self, days: u32) -> RpcResult<dreaming::Metrics> {
        dreaming::rpc::metrics(days).await.map_err(dreaming_err)
    }

    async fn endless_settings_get(&self) -> RpcResult<EndlessConfig> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetEndlessSettings).await
    }

    async fn endless_settings_set(&self, config: EndlessConfig) -> RpcResult<EndlessConfig> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SetEndlessSettings { config }).await
    }

    async fn meeting_oracle_mode(&self) -> RpcResult<decision::ModeInfo> {
        self.decision_mode(meeting::oracle::PREFIX.to_owned()).await
    }

    async fn set_meeting_oracle_mode(&self, mode: decision::Mode) -> RpcResult<decision::ModeInfo> {
        self.set_decision_mode(meeting::oracle::PREFIX.to_owned(), mode)
            .await
    }

    async fn decision_stats(&self) -> RpcResult<Vec<decision::FamilyStats>> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::AllStats).await
    }

    async fn decision_loaded(&self) -> RpcResult<Vec<decision::LoadedInfo>> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::LoadedNow).await
    }

    async fn decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::GetTrain { prefix }).await
    }

    async fn set_decision_training(
        &self,
        prefix: String,
        on: bool,
    ) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::SetTrain { prefix, on })
            .await
    }

    async fn set_decision_share(
        &self,
        prefix: String,
        on: bool,
    ) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::SetShare { prefix, on })
            .await
    }

    async fn delete_decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(
            decision::DECISION_ACTOR,
            decision::DeleteTraining { prefix },
        )
        .await
    }

    async fn meeting_speaker_delete(&self, profile: meeting::ProfileId) -> RpcResult<()> {
        send(&meeting()?, meeting::DeleteSpeaker { profile }).await
    }

    async fn meeting_live_diarization(&self) -> RpcResult<bool> {
        send(&meeting()?, meeting::LiveDiarization).await
    }

    async fn set_meeting_live_diarization(&self, on: bool) -> RpcResult<bool> {
        send(&meeting()?, meeting::SetLiveDiarization { on }).await
    }

    async fn meeting_summarize(&self, id: meeting::MeetingId) -> RpcResult<()> {
        send(&meeting()?, meeting::Summarize { meeting: id }).await
    }

    async fn search_settings_get(&self) -> RpcResult<search_providers::SearchSettings> {
        let defaults = ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::GetDefaults).await?;
        let providers = ask::<search_providers::SearchProviders, _>(
            search_providers::SEARCH_PROVIDERS_ACTOR,
            search_providers::GetProviders,
        )
        .await?;
        Ok(search_providers::SearchSettings {
            providers,
            defaults,
        })
    }

    async fn search_settings_set(
        &self,
        settings: search_providers::SearchSettings,
    ) -> RpcResult<search_providers::SearchSettings> {
        let defaults = ask::<web::WebActor, _>(
            web::WEB_ACTOR,
            web::settings::SetDefaults(settings.defaults),
        )
        .await?;
        let providers = ask::<search_providers::SearchProviders, _>(
            search_providers::SEARCH_PROVIDERS_ACTOR,
            search_providers::SetProviders(settings.providers),
        )
        .await?;
        Ok(search_providers::SearchSettings {
            providers,
            defaults,
        })
    }

    async fn search_provider_test(
        &self,
        name: String,
        query: String,
    ) -> RpcResult<web::settings::ProviderProbe> {
        ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::Probe { name, query }).await
    }

    async fn search_status(&self) -> RpcResult<Vec<web::health::ProviderStatus>> {
        ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::Status).await
    }

    async fn search_test(
        &self,
        query: String,
        site: Option<String>,
        recent: Option<web::Recency>,
    ) -> RpcResult<web::settings::SearchTestResult> {
        let out = ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            tools::Invoke(tools::ToolCall {
                id: format!("search-test-{}", uuid::Uuid::now_v7()),
                name: "search_web".into(),
                args: serde_json::json!({ "query": query, "site": site, "recent": recent }),
                session: SessionId::new(),
                quiet: true,
            }),
        )
        .await?;
        Ok(web::settings::SearchTestResult {
            text: out.content,
            is_error: out.is_error,
            structured: out.structured.unwrap_or(serde_json::Value::Null),
        })
    }

    async fn meeting_post_retry(&self, id: meeting::MeetingId) -> RpcResult<()> {
        send(&meeting()?, meeting::PostRetry { meeting: id }).await
    }

    async fn meeting_summary(&self, id: meeting::MeetingId) -> RpcResult<String> {
        send(&meeting()?, meeting::GetSummary { meeting: id }).await
    }

    async fn brief_search(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<meeting::CachedBrief>> {
        send(&meeting()?, meeting::BriefSearch { query, limit }).await
    }

    async fn brief_list(
        &self,
        tag: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<meeting::CachedBrief>> {
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

    async fn get_schedule(&self, name: String) -> RpcResult<schedule::file::ScheduleFile> {
        send(&schedule()?, schedule::Get { name }).await
    }

    async fn put_schedule(
        &self,
        file: schedule::file::ScheduleFile,
    ) -> RpcResult<schedule::ScheduleInfo> {
        send(&schedule()?, schedule::Put(file)).await
    }

    async fn delete_schedule(&self, name: String) -> RpcResult<()> {
        send(&schedule()?, schedule::Delete { name }).await
    }

    async fn test_schedule(&self, name: String) -> RpcResult<schedule::Verdict> {
        send(&schedule()?, schedule::TestSchedule { name }).await
    }

    async fn test_schedule_file(
        &self,
        file: schedule::file::ScheduleFile,
    ) -> RpcResult<schedule::Verdict> {
        send(&schedule()?, schedule::TestFile(file)).await
    }

    // 67: every method is one `ask`; the actors answer `not built` until
    // their workers fill them (docs/design/67-contract.md).

    async fn mesh_status(&self) -> RpcResult<mesh::MeshStatus> {
        send(&mesh_actor()?, mesh::Status).await
    }

    async fn mesh_pairing_string(&self) -> RpcResult<String> {
        send(&mesh_actor()?, mesh::PairingString).await
    }

    async fn mesh_add(&self, pairing: String) -> RpcResult<mesh::Peer> {
        send(&mesh_actor()?, mesh::AddPeer(pairing)).await
    }

    async fn mesh_remove(&self, id: String) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::RemovePeer(mesh::EngineId(id))).await
    }

    async fn mesh_set_hub(&self, pref: mesh::HubPref) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::SetHubPref(pref)).await
    }

    // 79: sandbox VMs. Each method is one `ask` to the `SandboxActor` (crates/sandbox/src/actor.rs).

    async fn sandboxes_list(&self) -> RpcResult<Vec<sandbox::SandboxInfo>> {
        send(&sandbox()?, sandbox::List).await
    }

    async fn sandbox_preflight(&self) -> RpcResult<sandbox::Preflight> {
        send(&sandbox()?, sandbox::PreflightNow).await
    }

    async fn sandbox_install_runtime(&self) -> RpcResult<()> {
        send(&sandbox()?, sandbox::InstallRuntime).await
    }

    async fn sandbox_create(
        &self,
        spec: sandbox::SandboxCreate,
    ) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Create { spec, origin: None }).await
    }

    async fn sandbox_retry(&self, name: String) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Retry { name }).await
    }

    async fn sandbox_start(&self, name: String) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Start { name }).await
    }

    async fn sandbox_stop(
        &self,
        name: String,
        grace_secs: Option<u64>,
        force: Option<bool>,
    ) -> RpcResult<sandbox::SandboxInfo> {
        send(
            &sandbox()?,
            sandbox::Stop {
                name,
                grace_secs: grace_secs.unwrap_or(20),
                force: force.unwrap_or(false),
            },
        )
        .await
    }

    async fn sandbox_pause(&self, name: String) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Pause { name }).await
    }

    async fn sandbox_resume(&self, name: String) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Resume { name }).await
    }

    async fn sandbox_resize(
        &self,
        name: String,
        change: sandbox::ResourceChange,
    ) -> RpcResult<sandbox::ResizeOutcome> {
        send(&sandbox()?, sandbox::Resize { name, change }).await
    }

    async fn sandbox_set_shares(
        &self,
        name: String,
        shares: Vec<sandbox::Mount>,
    ) -> RpcResult<sandbox::Pending> {
        send(&sandbox()?, sandbox::SetShares { name, shares }).await
    }

    async fn sandbox_snapshot(
        &self,
        name: String,
        label: String,
        full: Option<bool>,
    ) -> RpcResult<sandbox::VmSnapshotInfo> {
        send(
            &sandbox()?,
            sandbox::Snapshot {
                name,
                label,
                full: full.unwrap_or(false),
            },
        )
        .await
    }

    async fn sandbox_restore(
        &self,
        name: String,
        id: sandbox::SnapshotId,
    ) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Restore { name, id }).await
    }

    async fn sandbox_destroy(&self, name: String, keep_snapshots: Option<bool>) -> RpcResult<()> {
        send(
            &sandbox()?,
            sandbox::Destroy {
                name,
                keep_snapshots: keep_snapshots.unwrap_or(true),
            },
        )
        .await
    }

    async fn sandbox_desktop_access(
        &self,
        name: String,
    ) -> RpcResult<sandbox::types::DesktopAccess> {
        send(&sandbox()?, sandbox::DesktopAccess { name }).await
    }

    async fn sandbox_metrics(&self, name: String) -> RpcResult<sandbox::VmMetrics> {
        send(&sandbox()?, sandbox::Metrics { name }).await
    }

    async fn sandbox_logs(
        &self,
        name: String,
        tail: Option<usize>,
    ) -> RpcResult<Vec<sandbox::LogLine>> {
        send(
            &sandbox()?,
            sandbox::Logs {
                name,
                tail: tail.unwrap_or(200),
            },
        )
        .await
    }

    async fn sandbox_takeover(&self, name: String, on: bool) -> RpcResult<u32> {
        send(&sandbox()?, sandbox::Takeover { name, on }).await
    }

    async fn sandbox_terminal(
        &self,
        ext: &Extensions,
        name: String,
        session: Option<SessionId>,
    ) -> RpcResult<serde_json::Value> {
        let session = match session {
            Some(s) => s,
            None => owner_root().await?,
        };
        let auth = self.authorize(ext, session).await?;
        let job: String = send(
            &sandbox()?,
            sandbox::Terminal {
                name,
                session,
                root: auth.resources.root.clone(),
            },
        )
        .await?;
        Ok(serde_json::json!({ "job": job, "session": session }))
    }

    // 79 §6.6 (H fills these; stubs so the method list is complete).

    async fn handoffs_list(&self) -> RpcResult<Vec<roster::attention::Handoff>> {
        send(&attention_actor()?, roster::attention::ListHandoffs).await
    }

    async fn handoff_resolve(&self, id: String, outcome: String) -> RpcResult<()> {
        send(
            &attention_actor()?,
            roster::attention::ResolveHandoff { id, outcome },
        )
        .await
    }

    async fn agents_list(&self) -> RpcResult<Vec<roster::AgentSummary>> {
        send(&roster_actor()?, roster::ListAgents).await
    }

    async fn agent_get(&self, name: String) -> RpcResult<roster::AgentFile> {
        send(&roster_actor()?, roster::GetAgent(name)).await
    }

    async fn desk_open(&self, name: String) -> RpcResult<SessionId> {
        send(&roster_actor()?, roster::OpenDesk(name)).await
    }

    async fn agent_put(&self, file: roster::AgentFile) -> RpcResult<()> {
        send(&roster_actor()?, roster::PutAgent(file)).await
    }

    async fn agent_pause(&self, name: String, paused: bool) -> RpcResult<()> {
        send(&roster_actor()?, roster::PauseAgent { name, paused }).await
    }

    async fn agent_approve(&self, ext: &Extensions, name: String) -> RpcResult<()> {
        let m = roster::ApproveAgent {
            name,
            actor: crate::Client::actor(ext),
        };
        send(&roster_actor()?, m).await
    }

    async fn agent_remove(&self, name: String, note: Option<String>) -> RpcResult<()> {
        send(&roster_actor()?, roster::RemoveAgent { name, note }).await
    }

    async fn tasks_list(&self, filter: tasks::TaskFilter) -> RpcResult<Vec<tasks::Task>> {
        send(&tasks_actor()?, tasks::ListTasks(filter)).await
    }

    async fn task_get(&self, id: String) -> RpcResult<tasks::Task> {
        send(&tasks_actor()?, tasks::GetTask(tasks::TaskId(id))).await
    }

    async fn task_create(&self, input: tasks::TaskCreate) -> RpcResult<tasks::Task> {
        let m = tasks::CreateTask {
            input,
            created_by: "user".into(),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }

    async fn task_update(&self, id: String, patch: tasks::TaskPatch) -> RpcResult<tasks::Task> {
        let m = tasks::UpdateTask {
            id: tasks::TaskId(id),
            patch,
            actor: "user".into(),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }

    async fn task_move(
        &self,
        ext: &Extensions,
        id: String,
        state: tasks::TaskState,
        note: Option<String>,
        next: Option<u32>,
    ) -> RpcResult<tasks::Task> {
        let m = tasks::MoveTask {
            id: tasks::TaskId(id),
            state,
            note,
            done_because: None,
            next,
            actor: crate::Client::actor(ext),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }

    async fn task_comment(&self, id: String, text: String) -> RpcResult<()> {
        let m = tasks::CommentTask {
            id: tasks::TaskId(id),
            text,
            progress: false,
            actor: "user".into(),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }

    async fn task_events(&self, id: String) -> RpcResult<Vec<tasks::TaskEvent>> {
        send(&tasks_actor()?, tasks::TaskEvents(tasks::TaskId(id))).await
    }

    async fn task_blocks(&self, id: String) -> RpcResult<Vec<tasks::TaskId>> {
        send(&tasks_actor()?, tasks::TaskBlocks(tasks::TaskId(id))).await
    }

    async fn activity_list(
        &self,
        filter: tasks::activity::ActivityFilter,
    ) -> RpcResult<Vec<tasks::activity::ActivityEntry>> {
        let actor = lookup::<tasks::activity::Activity>(tasks::activity::ACTIVITY_ACTOR)?;
        send(&actor, tasks::activity::ListActivity(filter)).await
    }

    async fn attention_list(&self) -> RpcResult<Vec<roster::attention::AttentionItem>> {
        send(&attention_actor()?, roster::attention::ListAttention).await
    }

    async fn attention_snooze(&self, id: String, until_ms: u64) -> RpcResult<()> {
        send(
            &attention_actor()?,
            roster::attention::SnoozeAttention { id, until_ms },
        )
        .await
    }

    async fn attention_dismiss(&self, id: String) -> RpcResult<()> {
        send(&attention_actor()?, roster::attention::DismissAttention(id)).await
    }

    async fn resume_turn(&self, ext: &Extensions, session: SessionId) -> RpcResult<bool> {
        self.authorize(ext, session).await?;
        // 139 §9.2 (D18): an action, so it runs on the owner (I1). A desk's
        // press is forwarded; a user session is taken here (`Write`) and —
        // Revision 1 Q15 — the press *is* the user's decision, so the take is
        // followed by `ResumeTurn{Takeover, from}`. Idempotent against W2's
        // own resume-on-take: a turn already in flight answers "resumed lazily".
        let from = forward::owner_peer(session).await;
        if let Some(r) = forward::route(ext, session, "resume_turn", (session,)).await? {
            return Ok(r);
        }
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        let Some(turn) = send(&agents, agent::supervisor::OpenTurn(session)).await? else {
            return Ok(false);
        };
        // `cause: None` = the boot rule from the journal (§6.3) on the owner.
        send(
            &agents,
            agent::supervisor::ResumeTurn {
                session,
                turn,
                cause: from
                    .is_some()
                    .then_some(agent::durability::ResumeCause::Takeover),
                from: from.map(|p| p.0),
            },
        )
        .await?;
        Ok(true)
    }

    async fn packet_get(&self, key: String) -> RpcResult<Option<tasks::packet::Packet>> {
        send(&tasks_actor()?, tasks::GetPacket(key)).await
    }

    async fn task_decide(
        &self,
        ext: &Extensions,
        decision: String,
        answer: String,
    ) -> RpcResult<()> {
        let m = tasks::AnswerDecision {
            decision,
            answer,
            actor: crate::Client::actor(ext),
            session: None,
        };
        send(&tasks_actor()?, m).await
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

    async fn publish_app(
        &self,
        session: SessionId,
        name: String,
        source: String,
    ) -> RpcResult<AppDoc> {
        send(
            &apps()?,
            PublishFromUi {
                session,
                name,
                source,
            },
        )
        .await
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
        match send(
            &artifact()?,
            artifact::search::Get {
                call_id: call_id.clone(),
            },
        )
        .await
        {
            Ok(a) => Ok(a),
            // 68 §7: the index is engine-local; a record shown elsewhere comes from a peer.
            Err(e) => artifact::mesh::fetch_record(&self.rt.data_dir, &call_id, None)
                .await
                .map_err(|_| e),
        }
    }

    async fn get_artifact_listing(
        &self,
        call_id: String,
    ) -> RpcResult<artifact::archive::ArchiveListing> {
        let a = send(&artifact()?, artifact::search::Get { call_id }).await?;
        if a.kind != artifact::Kind::Archive {
            return Err(ErrorObjectOwned::owned(
                ASK_FAILED,
                format!("{}: not an archive", a.name),
                None::<()>,
            ));
        }
        artifact::archive::read_listing(&a.path)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e, None::<()>))
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

    async fn answer_secret(
        &self,
        id: secrets::RequestId,
        value: Option<String>,
        values: Option<HashMap<String, String>>,
    ) -> RpcResult<bool> {
        send(
            &secrets()?,
            secrets::Answer {
                id,
                value,
                values: values.unwrap_or_default(),
            },
        )
        .await
    }

    async fn secret_requests_list(&self) -> RpcResult<Vec<secrets::SecretRequest>> {
        send(
            &secrets()?,
            secrets::ListRequests {
                unanswered_only: true,
            },
        )
        .await
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
        let real = match std::fs::canonicalize(&path) {
            Ok(p) => p,
            // 68 §7: a blob (`…/artifacts/<hash>.<ext>`), spill file
            // (`…/spill/<session>/<call_id>.txt`) or attachment
            // (`…/attachments/<sha16>.<ext>`) this engine never wrote is fetched
            // from a peer — the session's owner first — and cached under ours.
            Err(e) => {
                let prefer = match session {
                    Some(s) => forward::owner_peer(s).await,
                    None => None,
                };
                let prefer = prefer.as_ref();
                let dir = |n: usize| {
                    path.ancestors()
                        .nth(n)
                        .and_then(Path::file_name)
                        .and_then(|d| d.to_str())
                };
                let fetched = match (path.file_name().and_then(|n| n.to_str()), dir(1), dir(2)) {
                    (Some(name), Some("artifacts"), _) => {
                        artifact::mesh::fetch_blob(&root.join("artifacts"), name, prefer).await
                    }
                    (Some(name), Some("attachments"), _) => {
                        artifact::mesh::fetch_attachment(&root.join("attachments"), name, prefer)
                            .await
                    }
                    (Some(_), Some(session), Some("spill")) => {
                        match (session.parse(), path.file_stem().and_then(|s| s.to_str())) {
                            (Ok(session), Some(call_id)) => {
                                artifact::mesh::fetch_spill(&root, session, call_id, prefer).await
                            }
                            _ => return Err(failed(format!("{}: {e}", path.display()))),
                        }
                    }
                    _ => return Err(failed(format!("{}: {e}", path.display()))),
                }
                .map_err(|e| failed(format!("{}: {e:#}", path.display())))?;
                std::fs::canonicalize(&fetched)
                    .map_err(|e| failed(format!("{}: {e}", fetched.display())))?
            }
        };
        let allowed = ["attachments", "browser", "computer", "artifacts", "spill"]
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

    async fn doc_subscribe(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        doc: document::DocId,
    ) -> SubscriptionResult {
        // Admission and the actor's subscription before accept, so the client's first
        // `doc_open` after this cannot race an update it would then miss (59 §6.2).
        let admit = async {
            let (peer, by) = principal(ext)?;
            let r = doc_send(&documents()?, document::Subscribe { doc, by }).await;
            member_outcome(&peer, &r);
            r
        };
        let mut rx = match admit.await {
            Ok(rx) => rx,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        tokio::spawn(async move {
            loop {
                let ev = tokio::select! {
                    _ = sink.closed() => return,
                    ev = rx.recv() => match ev { Some(ev) => ev, None => return },
                };
                let Ok(raw) = serde_json::value::to_raw_value(&ev) else {
                    continue;
                };
                let closed = matches!(ev, document::DocEvent::Closed { .. });
                if sink.send(SubscriptionMessage::from(raw)).await.is_err() || closed {
                    return;
                }
            }
        });
        Ok(())
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
        // 68 §5: a session owned elsewhere streams its live events here while
        // this subscription lives (the guard drops with the task).
        let follow = session.and_then(sessionsync::events::follow);
        let session = session.map(|s| s.to_string());
        let private = self.private.clone();
        // The snapshot goes first, always (empty too): the client replaces its
        // pending set with it, so a card whose `*Resolved` was missed in a
        // relay gap is dropped and one whose `*Requested` was missed appears.
        let snapshot = PendingPrompts::collect(|v| {
            wanted(v, session.as_deref(), tree.as_mut()) && admitted(v, peer, &private)
        })
        .await;
        if let Ok(raw) = serde_json::value::to_raw_value(&snapshot)
            && sink.send(SubscriptionMessage::from(raw)).await.is_err()
        {
            return Ok(());
        }
        // 126 §4.4: the chips and countdowns of pending cards follow the snapshot.
        if let Ok(Some(d)) = ActorRef::<double::actor::Double>::lookup(double::DOUBLE_ACTOR) {
            let _ = d.tell(double::actor::Republish).await;
        }
        // 129 §3.1: the ring is read by one task and the sink written by
        // another, joined by `Stream`: a client that stops reading never makes
        // `rx.recv()` fall behind; what waits merges (deltas) or queues
        // (`BACKLOG`, counted into `Lagged` on overflow).
        let state = Arc::new(parking_lot::Mutex::new(Stream {
            backlog: Backlog::default(),
            tree,
            closed: false,
        }));
        let notify = Arc::new(tokio::sync::Notify::new());
        let focus = self.focus.clone();
        let read = {
            let (state, notify, session, private) = (
                state.clone(),
                notify.clone(),
                session.clone(),
                private.clone(),
            );
            async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let pass = wanted(&ev, session.as_deref(), state.lock().tree.as_mut());
                            if !pass || !admitted(&ev, peer, &private) {
                                continue;
                            }
                            if is_delta(&ev) {
                                let focus = focus.read().get(&peer).cloned();
                                if let Some(set) = focus
                                    && !ev
                                        .get("session")
                                        .and_then(|s| s.as_str())
                                        .is_some_and(|s| set.contains(s))
                                {
                                    continue;
                                }
                            }
                            let now = Instant::now();
                            state.lock().backlog.push(ev, now);
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            state.lock().backlog.lost += n
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            state.lock().closed = true;
                            notify.notify_one();
                            return;
                        }
                    }
                    notify.notify_one();
                }
            }
        };
        let write = async move {
            loop {
                let now = Instant::now();
                let next = state.lock().backlog.next(now);
                match next {
                    Next::Frame(ev) => {
                        if let Ok(raw) = serde_json::value::to_raw_value(&*ev)
                            && sink.send(SubscriptionMessage::from(raw)).await.is_err()
                        {
                            return;
                        }
                    }
                    Next::Lagged(n) => {
                        // Dropped events may include a `*Resolved`: the snapshot after
                        // `Lagged` puts the client's pending set right again.
                        let lagged = serde_json::json!({ "type": "Lagged", "count": n });
                        if let Ok(raw) = serde_json::value::to_raw_value(&lagged)
                            && sink.send(SubscriptionMessage::from(raw)).await.is_err()
                        {
                            return;
                        }
                        let snapshot = PendingPrompts::collect(|v| {
                            let pass = wanted(v, session.as_deref(), state.lock().tree.as_mut());
                            pass && admitted(v, peer, &private)
                        })
                        .await;
                        if let Ok(raw) = serde_json::value::to_raw_value(&snapshot)
                            && sink.send(SubscriptionMessage::from(raw)).await.is_err()
                        {
                            return;
                        }
                    }
                    Next::Wait(at) => {
                        let closed = state.lock().closed;
                        if closed {
                            return;
                        }
                        tokio::select! {
                            _ = sink.closed() => return,
                            _ = notify.notified() => {}
                            _ = tokio::time::sleep_until(at.unwrap_or(now)), if at.is_some() => {}
                        }
                    }
                }
            }
        };
        tokio::spawn(async move {
            let _follow = follow;
            let reader = tokio::spawn(read);
            write.await;
            reader.abort();
        });
        Ok(())
    }

    async fn set_event_focus(&self, ext: &Extensions, sessions: Vec<SessionId>) -> RpcResult<()> {
        let peer = peer(ext)?;
        let set: Arc<HashSet<String>> =
            Arc::new(sessions.iter().map(ToString::to_string).collect());
        let fresh = self.focus.write().insert(peer.id(), set).is_none();
        if fresh {
            // The entry dies with the connection; a re-insert after that spawns
            // another waiter on the already-cancelled token, which removes at once.
            let focus = self.focus.clone();
            tokio::spawn(async move {
                peer.closed().await;
                focus.write().remove(&peer.id());
            });
        }
        Ok(())
    }
}

/// 129 §3.1: the event types a connection's focus filters and a stream merges
/// while they wait. Everything else passes untouched.
fn is_delta(ev: &serde_json::Value) -> bool {
    matches!(
        ev.get("type").and_then(|t| t.as_str()),
        Some("TextDelta" | "ReasoningDelta" | "ToolCallProgress" | "VisionProgress")
    )
}

/// Pending deltas go out at least this often when the sink has room (129 §3.1).
const FLUSH_MS: u64 = 40;

/// Non-delta events one stream holds for a slow client; past this the oldest
/// is dropped and counted into the next `Lagged` (129 §3.3).
const BACKLOG: usize = 8192;

/// What one `subscribe_events` stream shares between its ring reader and its
/// sink writer; locked per statement, never across an await.
struct Stream {
    backlog: Backlog,
    /// Descendants-mode tree; `wanted` enrols spawned children into it.
    tree: Option<HashSet<String>>,
    /// The ring is gone: the writer returns once the backlog is empty.
    closed: bool,
}

/// Frames of one stream between the ring and the sink, in emit order (129 §3.1,
/// §3.3). A delta merges into the latest frame of its key unless a non-delta
/// event of its session came after that frame, so per-session order is exact;
/// a delta batch waits up to `FLUSH_MS` unless a non-delta is queued behind it.
#[derive(Default)]
struct Backlog {
    frames: std::collections::VecDeque<Event>,
    /// Non-delta frames in `frames`, at most `BACKLOG`.
    control: usize,
    /// Delta frames in `frames`.
    deltas: usize,
    /// Delta frames of the batch currently going out.
    due: usize,
    /// When the pending delta batch is due.
    flush_at: Option<Instant>,
    /// Non-delta events dropped since the last `Lagged`.
    lost: u64,
}

/// What the sink writer does next.
enum Next {
    Frame(Event),
    Lagged(u64),
    /// Nothing due: wake on the next push, or at the instant.
    Wait(Option<Instant>),
}

impl Backlog {
    fn push(&mut self, ev: Event, now: Instant) {
        if !is_delta(&ev) {
            if self.control == BACKLOG {
                // ponytail: O(n) scan from the front; the oldest non-delta is near it.
                if let Some(i) = self.frames.iter().position(|f| !is_delta(f)) {
                    self.frames.remove(i);
                }
                self.control -= 1;
                self.lost += 1;
            }
            self.control += 1;
            self.frames.push_back(ev);
            return;
        }
        let same = |a: &serde_json::Value, b: &serde_json::Value, k: &str| a.get(k) == b.get(k);
        for f in self.frames.iter_mut().rev() {
            if !same(f, &ev, "session") {
                continue;
            }
            if !is_delta(f) {
                break;
            }
            if same(f, &ev, "type") && same(f, &ev, "turn") && same(f, &ev, "call_id") {
                // Text concatenates; `ToolCallProgress.args` and `VisionProgress`
                // are cumulative, so the latest replaces.
                if let Some(add) = ev.get("text").and_then(|t| t.as_str()) {
                    if let Some(serde_json::Value::String(acc)) = Arc::make_mut(f).get_mut("text") {
                        acc.push_str(add);
                    }
                } else {
                    *f = ev;
                }
                return;
            }
        }
        if self.deltas == 0 {
            self.flush_at = Some(now + Duration::from_millis(FLUSH_MS));
        }
        self.deltas += 1;
        self.frames.push_back(ev);
    }

    fn next(&mut self, now: Instant) -> Next {
        if self.lost > 0 {
            return Next::Lagged(std::mem::take(&mut self.lost));
        }
        let Some(delta) = self.frames.front().map(|f| is_delta(f)) else {
            return Next::Wait(None);
        };
        if delta {
            if self.due == 0 && self.control == 0 {
                if let Some(at) = self.flush_at
                    && at > now
                {
                    return Next::Wait(Some(at));
                }
                // The batch is due: everything pending goes; later arrivals
                // start the next one.
                self.due = self.deltas;
                self.flush_at = Some(now + Duration::from_millis(FLUSH_MS));
            }
            self.deltas -= 1;
            self.due = self.due.saturating_sub(1);
            if self.deltas == 0 {
                self.flush_at = None;
            }
        } else {
            self.control -= 1;
        }
        Next::Frame(self.frames.pop_front().expect("front exists"))
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

    /// A diagram's "Save SVG…" goes through `put_attachment` on the desktop shell:
    /// `.svg` must be accepted (image cap), unknown extensions still refused.
    #[test]
    fn decode_attachment_accepts_svg_and_refuses_unknown() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"<svg/>");
        assert_eq!(decode_attachment(b64.clone(), "svg").unwrap(), b"<svg/>");
        let err = decode_attachment(b64, "exe").unwrap_err();
        assert!(
            err.message().contains("unsupported attachment type .exe"),
            "{err}"
        );
    }

    /// 109 §10.3 wire shapes the desktop's bindings rely on: the interrupted
    /// history row, `Loaded.resume` (absent → `None`), the `resume` attention
    /// kind, and `resume_turn` registered on the module.
    #[tokio::test]
    async fn durability_wire_shapes() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-06T12:05:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let row = agent::HistoryEntry::Interrupted {
            text: "half a re".into(),
            at,
            context: None,
        };
        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            serde_json::json!({ "role": "interrupted", "text": "half a re", "at": "2026-10-06T12:05:00Z", "context": null })
        );
        let turn = runtime::TurnId::new();
        let loaded = agent::Loaded {
            resume: Some(agent::ResumeState {
                turn,
                kind: agent::durability::ResumeKind::Suspended,
                interrupted_at: at,
                resumed: false,
                report: None,
                owner: Some("engine-a".into()),
                here: false,
                cause: Some(agent::durability::ResumeCause::Transfer),
            }),
            ..Default::default()
        };
        let v = serde_json::to_value(&loaded).unwrap();
        assert_eq!(v["resume"]["kind"], "suspended");
        assert_eq!(v["resume"]["turn"], turn.to_string());
        // 139 §9.1: the replica's banner fields, snake-case cause.
        assert_eq!(v["resume"]["owner"], "engine-a");
        assert_eq!(v["resume"]["here"], false);
        assert_eq!(v["resume"]["cause"], "transfer");
        let back: agent::Loaded = serde_json::from_value(v).unwrap();
        assert_eq!(back.resume, loaded.resume);
        let mut pre109 = serde_json::to_value(agent::Loaded::default()).unwrap();
        pre109.as_object_mut().unwrap().remove("resume");
        let old: agent::Loaded = serde_json::from_value(pre109).unwrap();
        assert_eq!(old.resume, None, "a pre-109 engine's page still parses");
        // 139 §5.2: a pre-139 engine's `resume` reads as owned here, cause unknown.
        let pre139: agent::ResumeState = serde_json::from_value(serde_json::json!({
            "turn": turn, "kind": "interrupted", "interrupted_at": at, "resumed": true
        }))
        .unwrap();
        assert_eq!(
            (pre139.owner, pre139.here, pre139.cause),
            (None, true, None)
        );
        // 139 §9.1: `TurnResumed` carries `cause` + `from`; an old engine's omits both.
        let ev = agent::AgentEvent::TurnResumed {
            session: SessionId::new(),
            turn,
            kind: agent::durability::ResumeKind::Interrupted,
            interrupted_at: at,
            resumed_at: at,
            partial_dropped: false,
            cause: agent::durability::ResumeCause::Takeover,
            from: Some("engine-a".into()),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            (&v["cause"], &v["from"]),
            (
                &serde_json::json!("takeover"),
                &serde_json::json!("engine-a")
            )
        );
        let mut old = v.clone();
        let o = old.as_object_mut().unwrap();
        o.remove("cause");
        o.remove("from");
        match serde_json::from_value::<agent::AgentEvent>(old).unwrap() {
            agent::AgentEvent::TurnResumed { cause, from, .. } => {
                assert_eq!((cause, from), (agent::durability::ResumeCause::Crash, None));
            }
            other => panic!("{other:?}"),
        }
        // 139 §6.1: the SESSION_DRAINING payload in the external enum form the composer keys on.
        let session = SessionId::new();
        let e = session_draining(
            session,
            ErrorObjectOwned::owned(ASK_FAILED, "SessionDraining:release:engine-b", None::<()>),
        );
        assert_eq!(e.code(), forward::SESSION_DRAINING);
        assert_eq!(
            e.message(),
            format!("{session} is moving to engine-b — retry in a moment")
        );
        let data: serde_json::Value = serde_json::from_str(e.data().unwrap().get()).unwrap();
        assert_eq!(
            data,
            serde_json::json!({ "SessionDraining": { "session": session, "reason": "release", "next_owner": "engine-b" } })
        );
        let e = session_draining(
            session,
            ErrorObjectOwned::owned(ASK_FAILED, "SessionDraining:shutdown", None::<()>),
        );
        assert_eq!(e.code(), forward::SESSION_DRAINING);
        assert_eq!(
            e.message(),
            format!("{session} is pausing for shutdown — resumes on the next start")
        );
        let other = ErrorObjectOwned::owned(ASK_FAILED, "runner unavailable", None::<()>);
        assert_eq!(
            session_draining(session, other.clone()),
            other,
            "anything else passes through"
        );
        assert_eq!(
            serde_json::to_value(roster::attention::AttentionKind::Resume).unwrap(),
            "resume"
        );
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let names: Vec<&str> = Server::new(rt).into_rpc().method_names().collect();
        assert!(names.contains(&"resume_turn"), "{names:?}");
    }

    /// 136 §2.7: every `dreaming.*` method is registered under its dotted name.
    #[tokio::test]
    async fn dreaming_methods_are_registered() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let names: Vec<&str> = Server::new(rt).into_rpc().method_names().collect();
        for m in dreaming::rpc::METHODS {
            assert!(names.contains(&m), "{m} missing from {names:?}");
        }
    }

    /// 139 §9.2 (D18): the owner side of a routed `resume_turn` / `answer_ask` /
    /// `resolve_approval` — dispatched exactly as `forward::serve` does (internal
    /// peer + `Forwarded` marker) with the positional params the forwarder sends.
    /// Past the marker the call is local and reaches the actor lookup (no actors
    /// here → `NOT_RUNNING` / `ASK_FAILED`), never a second hop and never
    /// `INVALID_PARAMS`.
    #[tokio::test]
    async fn routed_methods_dispatch_on_the_owner_with_the_forwarders_params() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
        methods.extensions_mut().insert(link::RpcPeer::internal());
        methods
            .extensions_mut()
            .insert(forward::Forwarded(replica::PeerId("engine-b".into())));
        let session = SessionId::new();
        let ask_id = AskId(Uuid::now_v7());
        let approval = ApprovalId(Uuid::now_v7());
        let answers: Option<Vec<Answer>> = None;
        for (method, params, code) in [
            (
                "resume_turn",
                serde_json::to_string(&(session,)).unwrap(),
                NOT_RUNNING,
            ),
            (
                "answer_ask",
                serde_json::to_string(&(ask_id, &answers)).unwrap(),
                ASK_FAILED,
            ),
            (
                "resolve_approval",
                serde_json::to_string(&(approval, Decision::Allow)).unwrap(),
                NOT_RUNNING,
            ),
        ] {
            let req =
                format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params},"id":1}}"#);
            let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
            let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(i64::from(code)),
                "{req} → {v}"
            );
        }
    }

    /// 139 §9.2 (D18), R1 §8.11: an answer to a question whose replicated row
    /// names a session owned elsewhere goes to that owner — forward, never take
    /// — and is `-32010` when the owner is not connected; a question of our own
    /// session, or one with no row, is answered locally.
    #[tokio::test]
    async fn answers_follow_the_questions_row_to_its_owner_or_fail_unreachable() {
        use tools::journal::{QuestionKind, QuestionRow, QuestionState};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite");
        let (theirs_s, mine_s) = (SessionId::new(), SessionId::new());
        let (theirs_q, mine_q) = (Uuid::now_v7(), Uuid::now_v7());
        let question = |id: Uuid, session: SessionId| QuestionRow {
            id: id.to_string(),
            session,
            tool_call_id: None,
            kind: QuestionKind::Ask,
            payload: serde_json::json!({}),
            state: QuestionState::Pending,
            asked_at: chrono::Utc::now(),
            deadline: chrono::Utc::now(),
            answered_at: None,
            answer: None,
            answered_by: None,
        };
        let theirs = agent::store::Store::open(&path, Some(mesh::EngineId("them".into())))
            .await
            .unwrap();
        theirs
            .create_session(theirs_s, "m", None, None, None)
            .await
            .unwrap();
        theirs
            .question_put(&question(theirs_q, theirs_s))
            .await
            .unwrap();
        theirs.close().await;
        let store = agent::store::Store::open(&path, Some(mesh::EngineId("me".into())))
            .await
            .unwrap();
        store
            .create_session(mine_s, "m", None, None, None)
            .await
            .unwrap();
        store.question_put(&question(mine_q, mine_s)).await.unwrap();

        assert_eq!(
            question_owner(&store, &theirs_q).await.unwrap(),
            Some(replica::PeerId("them".into()))
        );
        assert_eq!(question_owner(&store, &mine_q).await.unwrap(), None);
        assert_eq!(question_owner(&store, &Uuid::now_v7()).await.unwrap(), None);

        let ext = Extensions::new();
        let local: Option<bool> = route_question_via(
            &ext,
            &store,
            &mine_q,
            "answer_ask",
            (AskId(mine_q), None::<Vec<Answer>>),
        )
        .await
        .unwrap();
        assert_eq!(local, None, "our own question: answered here");
        // No replica transport in this process: the owner is not connected.
        let e = route_question_via::<bool>(
            &ext,
            &store,
            &theirs_q,
            "answer_ask",
            (AskId(theirs_q), None::<Vec<Answer>>),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code(), forward::OWNER_UNREACHABLE);
        let data: serde_json::Value = serde_json::from_str(e.data().unwrap().get()).unwrap();
        assert_eq!(
            data,
            serde_json::json!({ "OwnerUnreachable": { "owner": "them", "last_seen": null } })
        );
        store.close().await;
    }
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

    /// 130 E1: the inbox-changed event is a tagged object every `subscribe_events(null)` client sees.
    #[test]
    fn attention_changed_wire_shape() {
        let ev = roster::attention::AttentionChanged {
            count: 2,
            ids_added: vec!["finished:s1:t9".into()],
            ids_removed: vec!["ask:a1".into()],
            at_ms: 1_700_000_000_000,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "type": "AttentionChanged", "count": 2, "ids_added": ["finished:s1:t9"], "ids_removed": ["ask:a1"], "at_ms": 1_700_000_000_000u64 })
        );
        assert!(
            wanted(&v, None, None),
            "session-less: every global subscriber"
        );
        assert!(wanted(&v, Some("other"), None));
        assert_eq!(
            serde_json::to_value(roster::attention::AttentionKind::Finished).unwrap(),
            "finished"
        );
    }

    /// LIVE-GLYPHS A2: the fired reminder reaches the desktop as a tagged
    /// object; a global subscriber sees it like `JobStarted`.
    #[test]
    fn reminder_fired_wire_shape() {
        let s = SessionId::new();
        let v = serde_json::to_value(tools::ReminderEvent::Fired {
            session: s,
            id: "r1".into(),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "type": "ReminderFired", "session": s, "id": "r1" })
        );
        assert!(wanted(&v, None, None));
        assert!(!is_delta(&v));
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

    /// 129 §3: focus filter, delta merging, bounded backlog, `Lagged`.
    mod event_focus {
        use super::*;
        use jsonrpsee::core::server::Subscription;
        use std::time::Duration;

        fn delta(kind: &str, s: SessionId, turn: &str, text: &str) -> serde_json::Value {
            serde_json::json!({ "type": kind, "session": s, "turn": turn, "text": text })
        }

        fn progress(s: SessionId, call: &str, args: &str) -> serde_json::Value {
            serde_json::json!({ "type": "ToolCallProgress", "session": s, "turn": "t", "call_id": call, "args": args })
        }

        fn title(s: SessionId, title: impl Display) -> serde_json::Value {
            serde_json::json!({ "type": "TitleChanged", "session": s, "title": title.to_string() })
        }

        /// Every frame until `Wait`, with the `Lagged` counts in place.
        fn drain(b: &mut Backlog, now: Instant) -> Vec<serde_json::Value> {
            let mut out = Vec::new();
            loop {
                match b.next(now) {
                    Next::Frame(ev) => out.push(serde_json::Value::clone(&ev)),
                    Next::Lagged(n) => {
                        out.push(serde_json::json!({ "type": "Lagged", "count": n }))
                    }
                    Next::Wait(_) => return out,
                }
            }
        }

        /// (b) Ten `TextDelta`s of one turn become one event; the `ToolCallStarted`
        /// behind them follows it; `ToolCallProgress` keeps the latest args; a
        /// non-delta of the same session splits the merge so order stays exact,
        /// while another session's deltas merge across it.
        #[test]
        fn backlog_merges_deltas_in_session_order() {
            let now = Instant::now();
            let (s, r) = (SessionId::new(), SessionId::new());
            let mut b = Backlog::default();
            for i in 0..10 {
                b.push(Arc::new(delta("TextDelta", s, "t", &i.to_string())), now);
            }
            b.push(Arc::new(delta("ReasoningDelta", s, "t", "think")), now);
            let started =
                serde_json::json!({ "type": "ToolCallStarted", "session": s, "turn": "t" });
            b.push(Arc::new(started.clone()), now);
            b.push(Arc::new(progress(s, "c", "{\"a")), now);
            b.push(Arc::new(progress(s, "c", "{\"a\":1}")), now);
            b.push(Arc::new(progress(s, "d", "{}")), now);
            b.push(Arc::new(delta("TextDelta", r, "u", "p")), now);
            b.push(Arc::new(title(s, "x")), now);
            b.push(Arc::new(delta("TextDelta", r, "u", "q")), now);
            b.push(Arc::new(delta("TextDelta", s, "t", "z")), now);
            let out = drain(&mut b, now);
            assert_eq!(out[0], delta("TextDelta", s, "t", "0123456789"));
            assert_eq!(out[1], delta("ReasoningDelta", s, "t", "think"));
            assert_eq!(out[2], started);
            assert_eq!(out[3], progress(s, "c", "{\"a\":1}"));
            assert_eq!(out[4], progress(s, "d", "{}"));
            assert_eq!(out[5], delta("TextDelta", r, "u", "pq"));
            assert_eq!(out[6], title(s, "x"));
            // Only deltas left and the batch is not due: they wait for `FLUSH_MS`.
            assert_eq!(out.len(), 7, "{out:?}");
            assert!(
                matches!(b.next(now), Next::Wait(Some(at)) if at == now + Duration::from_millis(FLUSH_MS))
            );
            let later = now + Duration::from_millis(FLUSH_MS);
            assert_eq!(drain(&mut b, later), [delta("TextDelta", s, "t", "z")]);
            assert!(matches!(b.next(later), Next::Wait(None)));
        }

        /// (d) Past `BACKLOG` non-delta events the oldest go, counted into one
        /// `Lagged` ahead of what is left; deltas never count.
        #[test]
        fn backlog_overflow_drops_oldest_and_counts() {
            let now = Instant::now();
            let s = SessionId::new();
            let mut b = Backlog::default();
            b.push(Arc::new(delta("TextDelta", s, "t", "a")), now);
            for i in 0..BACKLOG + 5 {
                b.push(Arc::new(title(s, i)), now);
            }
            for _ in 0..100 {
                b.push(Arc::new(delta("TextDelta", s, "t", "b")), now);
            }
            // Past the flush deadline so the trailing delta batch goes too.
            let out = drain(&mut b, now + Duration::from_millis(FLUSH_MS));
            assert_eq!(out[0], serde_json::json!({ "type": "Lagged", "count": 5 }));
            assert_eq!(out[1], delta("TextDelta", s, "t", "a"));
            assert_eq!(out[2], title(s, 5));
            assert_eq!(out[1 + BACKLOG], title(s, BACKLOG + 4));
            assert_eq!(
                out[2 + BACKLOG],
                delta("TextDelta", s, "t", &"b".repeat(100))
            );
            assert_eq!(out.len(), 3 + BACKLOG);
        }

        struct Harness {
            methods: jsonrpsee::Methods,
            tx: broadcast::Sender<Event>,
            focus: FocusMap,
            peer: link::RpcPeer,
            guard: Option<link::RpcPeerGuard>,
        }

        impl Harness {
            /// A server whose feed is a ring of `ring` slots, every call authenticated as one peer.
            fn new(ring: usize) -> Self {
                let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
                let mut server = Server::new(rt);
                let tx = broadcast::channel(ring).0;
                server.events = tx.clone();
                let focus = server.focus.clone();
                let mut methods: jsonrpsee::Methods = server.into_rpc().into();
                let (peer, guard) = link::RpcPeer::open();
                methods.extensions_mut().insert(peer.clone());
                Self {
                    methods,
                    tx,
                    focus,
                    peer,
                    guard: Some(guard),
                }
            }

            /// `subscribe_events(null, null)` with a sink buffer of `buf`; the snapshot is consumed.
            async fn stream(&self, buf: usize) -> Subscription {
                let mut sub = self
                    .methods
                    .subscribe(
                        "subscribe_events",
                        [serde_json::Value::Null, serde_json::Value::Null],
                        buf,
                    )
                    .await
                    .unwrap();
                assert_eq!(next(&mut sub).await["type"], "PendingPrompts");
                sub
            }

            async fn focus(&self, sessions: Vec<SessionId>) {
                self.methods
                    .call::<_, ()>("set_event_focus", (sessions,))
                    .await
                    .unwrap();
            }

            fn publish(&self, ev: serde_json::Value) {
                self.tx.send(Arc::new(ev)).unwrap();
            }
        }

        async fn next(sub: &mut Subscription) -> serde_json::Value {
            tokio::time::timeout(Duration::from_secs(5), sub.next::<serde_json::Value>())
                .await
                .expect("an event in time")
                .expect("stream open")
                .expect("json")
                .0
        }

        async fn quiet(sub: &mut Subscription) {
            let r =
                tokio::time::timeout(Duration::from_secs(1), sub.next::<serde_json::Value>()).await;
            assert!(r.is_err(), "unexpected event: {r:?}");
        }

        /// (a) With a focus set, deltas of other sessions are dropped and their
        /// non-delta events pass; `[]` drops every delta; a later call replaces
        /// the set on a stream opened before it; no call passes everything.
        #[tokio::test(start_paused = true)]
        async fn focus_filters_deltas_only() {
            let h = Harness::new(1024);
            let mut sub = h.stream(1024).await;
            let (a, b) = (SessionId::new(), SessionId::new());
            h.focus(vec![a]).await;
            h.publish(delta("TextDelta", a, "t", "x"));
            h.publish(delta("TextDelta", b, "t", "y"));
            h.publish(progress(b, "c", "{}"));
            h.publish(title(b, "T"));
            assert_eq!(next(&mut sub).await, delta("TextDelta", a, "t", "x"));
            assert_eq!(next(&mut sub).await, title(b, "T"));
            quiet(&mut sub).await;

            h.focus(vec![b]).await;
            h.publish(delta("TextDelta", a, "t", "x"));
            h.publish(delta("ReasoningDelta", b, "t", "r"));
            assert_eq!(next(&mut sub).await, delta("ReasoningDelta", b, "t", "r"));
            quiet(&mut sub).await;

            h.focus(vec![]).await;
            h.publish(delta("TextDelta", b, "t", "x"));
            h.publish(progress(b, "c", "{}"));
            h.publish(title(a, "U"));
            assert_eq!(next(&mut sub).await, title(a, "U"));
            quiet(&mut sub).await;

            let open = Harness::new(1024);
            let mut sub = open.stream(1024).await;
            open.publish(delta("TextDelta", a, "t", "x"));
            open.publish(progress(b, "c", "{}"));
            assert_eq!(next(&mut sub).await, delta("TextDelta", a, "t", "x"));
            assert_eq!(next(&mut sub).await, progress(b, "c", "{}"));
        }

        /// (c) A client that stops reading while 10 000 deltas arrive through a
        /// 1024-slot ring sees no `Lagged`: the ring is drained regardless, and
        /// the text reaches it whole in a few frames.
        #[tokio::test]
        async fn stalled_sink_merges_deltas_without_lag() {
            let h = Harness::new(1024);
            let mut sub = h.stream(1).await;
            let s = SessionId::new();
            let mut want = String::new();
            for i in 0..10_000 {
                let piece = format!("{i} ");
                want.push_str(&piece);
                h.publish(delta("TextDelta", s, "t", &piece));
                // The publisher's ring is read by the stream's own task.
                tokio::task::yield_now().await;
            }
            h.publish(title(s, "end"));
            let mut got = String::new();
            let mut frames = 0;
            loop {
                let ev = next(&mut sub).await;
                assert_ne!(ev["type"], "Lagged", "{ev}");
                if ev["type"] == "TitleChanged" {
                    break;
                }
                assert_eq!(ev["type"], "TextDelta");
                got.push_str(ev["text"].as_str().unwrap());
                frames += 1;
            }
            assert_eq!(got, want);
            assert!(frames <= 3, "{frames} frames");
        }

        /// (d) end to end: a stalled sink past `BACKLOG` non-delta events gets one
        /// `Lagged` whose count is exactly what is missing, then the snapshot,
        /// then the rest in order.
        #[tokio::test]
        async fn backlog_overflow_lags_once_then_snapshots() {
            let h = Harness::new(1 << 14);
            let mut sub = h.stream(1).await;
            let s = SessionId::new();
            let n = BACKLOG + 3;
            for i in 0..n {
                h.publish(title(s, i));
                tokio::task::yield_now().await;
            }
            let mut lagged = None;
            let mut titles = Vec::new();
            while titles.len() + lagged.unwrap_or(0) < n {
                let ev = next(&mut sub).await;
                match ev["type"].as_str().unwrap() {
                    "Lagged" => {
                        assert!(lagged.is_none(), "second Lagged");
                        lagged = Some(ev["count"].as_u64().unwrap() as usize);
                        assert_eq!(next(&mut sub).await["type"], "PendingPrompts");
                    }
                    "TitleChanged" => {
                        titles.push(ev["title"].as_str().unwrap().parse::<usize>().unwrap())
                    }
                    other => panic!("{other}"),
                }
            }
            let lost = lagged.expect("one Lagged");
            assert!(lost >= 1, "{lost}");
            assert!(titles.windows(2).all(|w| w[0] < w[1]));
            assert_eq!(*titles.last().unwrap(), n - 1);
        }

        /// (e) The focus entry dies with its connection.
        #[tokio::test]
        async fn focus_dies_with_the_connection() {
            let mut h = Harness::new(1024);
            let id = h.peer.id();
            h.focus(vec![SessionId::new()]).await;
            h.focus(vec![]).await;
            assert!(h.focus.read().get(&id).is_some_and(|set| set.is_empty()));
            drop(h.guard.take());
            for _ in 0..100 {
                if !h.focus.read().contains_key(&id) {
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("focus entry outlived its connection");
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

    /// 128 Rev 3: `spend_report` is registered, parses `[days]` and reaches
    /// the agent lookup (no actor here, so `NOT_RUNNING`); its reply type
    /// round-trips with `kind` on the wire as the snake-case string.
    #[tokio::test]
    async fn spend_report_is_registered_and_round_trips() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
        assert!(methods.method_names().any(|m| m == "spend_report"));
        let (resp, _) = methods
            .raw_json_request(
                r#"{"jsonrpc":"2.0","method":"spend_report","params":[7],"id":1}"#,
                1,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(NOT_RUNNING)));

        let report = SpendReport {
            days: vec![agent::SpendDay {
                day: "2026-10-07".into(),
                usd: 1302.38,
                calls: 3,
                unresolved: 1,
            }],
            month: agent::SpendDay {
                day: "2026-10".into(),
                usd: 4102.0,
                calls: 36_861,
                unresolved: 0,
            },
            owners: vec![agent::SpendOwner {
                kind: agent::SpendOwnerKind::Agent,
                name: "builder".into(),
                usd: 12.5,
                calls: 40,
            }],
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["owners"][0]["kind"], "agent");
        assert_eq!(json["days"][0]["day"], "2026-10-07");
        let back: SpendReport = serde_json::from_value(json).unwrap();
        assert_eq!(back, report);
    }

    /// The daemon's devices are out of a relay peer's reach: `talk_start` with
    /// `sink: device` is refused before any actor lookup (76 §5.1).
    #[tokio::test]
    async fn talk_device_sink_refuses_relay_peers() {
        let v = remote_dictation_call(
            "talk_start",
            &format!(r#"["{}","device",null]"#, SessionId::new()),
        )
        .await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(ASK_FAILED)));
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("speaker and microphone are on the harness host"))
        );
        // A client sink without a rate is refused for anyone.
        let v = remote_dictation_call(
            "talk_start",
            &format!(r#"["{}","client",null]"#, SessionId::new()),
        )
        .await;
        assert_eq!(
            v["error"]["message"].as_str(),
            Some("sink: client needs sample_rate")
        );
        // Well-formed: reaches the actor lookup (no talk actor here).
        let v = remote_dictation_call(
            "talk_start",
            &format!(r#"["{}","client",48000]"#, SessionId::new()),
        )
        .await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(NOT_RUNNING)));
        let v = remote_dictation_call("talk_status", "[]").await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(NOT_RUNNING)));
    }

    /// 120 §13.1: `page_preview` driven against the real `answer_page_preview`
    /// dispatch. Time is paused, so the 2 s / 15 s deadlines elapse at once.
    mod page_preview {
        use super::*;
        use std::time::Duration;

        use artifact::preview::{
            Client, ConsoleLevel, ConsoleLine, PagePreviewAnswer, PagePreviewRequested,
            PagePreviewResolved, PagePreviewTool, PreviewArgs, PreviewId,
        };
        use kameo::actor::Spawn;
        use kameo::message::Context;
        use tokio::sync::mpsc;
        use tools::{Tool, ToolCx, ToolOutput};

        /// Bus subscriber: the request ids and the resolved ids, in order.
        #[derive(kameo::Actor)]
        struct Capture(mpsc::UnboundedSender<(bool, PreviewId)>);

        impl Message<PagePreviewRequested> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: PagePreviewRequested, _: &mut Context<Self, ()>) {
                assert_eq!(m.html, "<p>x</p>");
                let _ = self.0.send((false, m.id));
            }
        }

        impl Message<PagePreviewResolved> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: PagePreviewResolved, _: &mut Context<Self, ()>) {
                let _ = self.0.send((true, m.id));
            }
        }

        struct Harness {
            rt: Runtime,
            methods: jsonrpsee::Methods,
            events: mpsc::UnboundedReceiver<(bool, PreviewId)>,
        }

        impl Harness {
            async fn new() -> Self {
                let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
                let methods: jsonrpsee::Methods = Server::new(rt.clone()).into_rpc().into();
                let (tx, events) = mpsc::unbounded_channel();
                let cap = Capture::spawn(Capture(tx));
                rt.subscribe::<Capture, PagePreviewRequested>(&cap).await;
                rt.subscribe::<Capture, PagePreviewResolved>(&cap).await;
                Self {
                    rt,
                    methods,
                    events,
                }
            }

            /// Run the tool for `session` on its own task; the width defaults.
            fn call(&self, session: SessionId) -> tokio::task::JoinHandle<ToolOutput> {
                let tool = PagePreviewTool {
                    rt: self.rt.clone(),
                };
                let cx = ToolCx {
                    session,
                    call_id: "c".into(),
                    quiet: false,
                    resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
                };
                tokio::spawn(async move {
                    Tool::call(
                        &tool,
                        PreviewArgs {
                            html: "<p>x</p>".into(),
                            ..Default::default()
                        },
                        cx,
                    )
                    .await
                    .unwrap()
                })
            }

            async fn requested(&mut self) -> PreviewId {
                let (resolved, id) = self.events.recv().await.unwrap();
                assert!(!resolved, "expected a request");
                id
            }

            async fn resolved(&mut self) -> PreviewId {
                let (resolved, id) = self.events.recv().await.unwrap();
                assert!(resolved, "expected a resolution");
                id
            }

            /// The raw `answer_page_preview` result.
            async fn answer(&self, id: PreviewId, answer: &PagePreviewAnswer) -> bool {
                let req = format!(
                    r#"{{"jsonrpc":"2.0","method":"answer_page_preview","params":[{},{}],"id":1}}"#,
                    serde_json::to_string(&id).unwrap(),
                    serde_json::to_string(answer).unwrap()
                );
                let (resp, _) = self.methods.raw_json_request(&req, 1).await.unwrap();
                let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
                v["result"].as_bool().unwrap_or_else(|| panic!("{v}"))
            }
        }

        fn started() -> PagePreviewAnswer {
            PagePreviewAnswer::Started {
                client: Client::Desktop,
            }
        }

        fn done(height: u32) -> PagePreviewAnswer {
            PagePreviewAnswer::Done {
                content_height: height,
                console: vec![ConsoleLine {
                    level: ConsoleLevel::Error,
                    text: "boom".into(),
                }],
                loaded: true,
                client: Client::Desktop,
            }
        }

        #[tokio::test(start_paused = true)]
        async fn answer_page_preview_first_done_wins() {
            let mut h = Harness::new().await;
            let call = h.call(SessionId::new());
            let id = h.requested().await;
            assert!(h.answer(id, &started()).await);
            assert!(h.answer(id, &done(612)).await);
            assert!(!h.answer(id, &done(700)).await, "second Done is late");
            assert!(
                !h.answer(id, &started()).await,
                "Started after Done is late"
            );
            let out = call.await.unwrap();
            assert!(!out.is_error, "{}", out.content);
            assert!(out.content.starts_with(
                "Previewed at 728 px: contentHeight 612, 1 error. Pass height: 612 to show"
            ));
            assert_eq!(out.structured.unwrap()["contentHeight"], 612);
            assert_eq!(h.resolved().await, id);
        }

        #[tokio::test(start_paused = true)]
        async fn started_extends_the_deadline() {
            let mut h = Harness::new().await;
            let call = h.call(SessionId::new());
            let id = h.requested().await;
            assert!(h.answer(id, &started()).await);
            // Past the 2 s "no renderer" mark, inside the 15 s total.
            tokio::time::sleep(Duration::from_secs(5)).await;
            assert!(h.answer(id, &done(300)).await);
            let out = call.await.unwrap();
            assert!(!out.is_error, "{}", out.content);

            // A `Started` with no `Done` runs out at 15 s.
            let call = h.call(SessionId::new());
            h.resolved().await;
            let id = h.requested().await;
            assert!(h.answer(id, &started()).await);
            let out = call.await.unwrap();
            assert!(out.is_error);
            assert!(
                out.content
                    .starts_with("page_preview: preview timed out after 15 s"),
                "{}",
                out.content
            );
            assert!(!h.answer(id, &done(1)).await, "dead after the timeout");
        }

        #[tokio::test(start_paused = true)]
        async fn no_started_in_two_seconds_is_no_renderer() {
            let mut h = Harness::new().await;
            let call = h.call(SessionId::new());
            let id = h.requested().await;
            let out = call.await.unwrap();
            assert!(out.is_error);
            assert!(
                out.content
                    .starts_with("page_preview: no renderer connected — open the theuth desktop"),
                "{}",
                out.content
            );
            assert_eq!(h.resolved().await, id);
            assert!(!h.answer(id, &started()).await);
        }

        #[tokio::test(start_paused = true)]
        async fn one_preview_per_session() {
            let mut h = Harness::new().await;
            let session = SessionId::new();
            let first = h.call(session);
            let id = h.requested().await;
            let out = h.call(session).await.unwrap();
            assert!(out.is_error);
            assert_eq!(out.content, "page_preview: a preview is already running");
            // Another session is not blocked.
            let other = h.call(SessionId::new());
            let other_id = h.requested().await;
            assert_ne!(other_id, id);
            assert!(h.answer(id, &done(100)).await);
            assert!(h.answer(other_id, &done(200)).await);
            assert!(!first.await.unwrap().is_error);
            assert!(!other.await.unwrap().is_error);
        }
    }
}
