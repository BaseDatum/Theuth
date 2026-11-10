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
use agent::SetModelFavorite;
use agent::supervisor::{ChildResumed, HarnessOptions, ResumeChild, SetHarnessOption};
use agent::todo::TodoSnapshot;
use agent::{
    AddAcp, AddProvider, AddSkill, AddSkillRepo, AgentInfo, Agents, AnswerPeerRequest,
    ArchiveSession, Broadcast, BroadcastOutcome, BroadcastReceipt, BroadcastRequest,
    BroadcastTargets, Cancel, CancelScope, CommunicationState, Compact, CompactStyle,
    ContextBreakdown, ContextItems, ContextRemoval, ContextSnapshot, ContextTarget, DeleteSession,
    Delivery, EditTurn, EscalateModel, ForkSession, GetCommunication, GetCompactStyle,
    GetEscalateModel, GetPeerMessage, GetSubagentModel, GetTitleModel, GetTodos, GetVisionModel,
    Harvest, ListAcp, ListActivity, ListAgents, ListLogins, ListModels, ListPeerMessages,
    ListPeers, ListProviders, ListQueued, ListRegistry, ListSessions, ListSkills, LoadSession,
    LoadSkill, Loaded, Login, LoginInfo, Logout, ModelInfo, NewRootSession, PeerMessagePage,
    PeerPage, PeerReceipt, PeerRecord, PeerScope, PinSession, ProviderInfo, QueuedInput, Redo,
    RefreshProviderModels, RefreshedModels, Regenerate, RemoveAcp, RemoveContext, RemoveProvider,
    RemoveSkill, RemoveSkillRepo, RenameSession, SendInput, SessionContextBreakdown,
    SessionSummary, SetCommunication, SetCompactStyle, SetEffort, SetEscalateModel, SetMaxSalvages,
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
use agent::{ContactDeliver, ContactEnvelope};
use agent::{EndlessConfig, MemoryMode, SessionMemory, ViewLine};
use agent::{GetPolishModel, Polish, PolishReply, Rewrite, SetPolishModel};
use app::{
    AppDoc, AppToolResult, Apps, CallArgs, CallStatusOf, CallTool, CloseApp, GetApp, PublishFromUi,
};
use artifact::search::ArtifactMatchContext;
use base64::Engine as _;
use browser::{BROWSER_ACTOR, Browsers, Screenshot};
use codeindex::{
    CODEINDEX_ACTOR, ChangeLens, CodeindexActor, FitLens, HereLens, IndexStats, LensMeta,
    LocateAnchor, Located, MapElement, MapExport, MapHit, MapIndex, MapJob,
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
pub(crate) const ASK_FAILED: i32 = -32000;
/// The named actor is not registered.
pub(crate) const NOT_RUNNING: i32 = -32001;
/// The call reached the method without an authenticated connection behind it.
pub(crate) const UNAUTHORIZED: i32 = -32002;

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
    /// `tasks.home_offline_ask_secs` (182 D9): how long a task's home machine may be
    /// offline, with tasks waiting on it, before the Attention item asks. `0` from an older harness.
    #[serde(default)]
    pub home_offline_ask_secs: u64,
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
    /// 183 W-12: `engine` (mesh id) lists on that engine when `session` is
    /// `None` (New chat on a machine); a missing trailing param is `None`.
    #[method(name = "path_complete", with_extensions)]
    async fn path_complete(
        &self,
        session: Option<SessionId>,
        input: String,
        cursor: usize,
        engine: Option<String>,
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
    /// 183 §7.10 (W-40): a contact's message for an exported chat owned here, forwarded by
    /// the engine that holds the contact. Mesh-internal like `peer_deliver`: a call without
    /// the `Forwarded` marker is `-32002`. Returns the owner's receipt.
    #[method(name = "contact_deliver", with_extensions)]
    async fn contact_deliver(&self, envelope: ContactEnvelope) -> RpcResult<PeerReceipt>;
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
    /// 186 (D8): `reason` `input` (the owner's interject or flush) is refused while the turn is a
    /// contact's; `stop` (the default) is an explicit Stop.
    #[method(name = "cancel", with_extensions)]
    async fn cancel(
        &self,
        session: SessionId,
        scope: Option<CancelScope>,
        reason: Option<agent::CancelReason>,
    ) -> RpcResult<()>;
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
    /// 137F: reset the backoff of every failed view part, re-resolve the
    /// compactor, wake the pump; the fresh `SessionMemory` (an epochs session's unchanged).
    #[method(name = "endless_retry", with_extensions)]
    async fn endless_retry(&self, session: SessionId) -> RpcResult<SessionMemory>;
    /// 137G: fold every unbuilt level-0 line mechanically (an excerpt, no model
    /// call, instant); the fresh `SessionMemory` (an epochs session's unchanged).
    #[method(name = "endless_fold", with_extensions)]
    async fn endless_fold(&self, session: SessionId) -> RpcResult<SessionMemory>;
    /// 137G: `paused = true` stops the session's compactor calls (in-flight ones
    /// cancelled, the backlog kept; persisted), `false` resumes; the fresh `SessionMemory`.
    #[method(name = "endless_pause", with_extensions)]
    async fn endless_pause(&self, session: SessionId, paused: bool) -> RpcResult<SessionMemory>;
    /// 137G: this chat's own compactor model (`None` = inherit the chain: agent
    /// default → `agent.endless.model` → title model → session model); the id
    /// must resolve; persisted, live; the fresh `SessionMemory`.
    #[method(name = "set_session_compactor_model", with_extensions)]
    async fn set_session_compactor_model(
        &self,
        session: SessionId,
        model: Option<String>,
    ) -> RpcResult<SessionMemory>;
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
    /// The live polish model (`voice.polish_model`, 153 §9.2), if set.
    #[method(name = "polish_model")]
    async fn polish_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `voice.polish_model`; `None` clears it (polish uses the title model, then the session's).
    #[method(name = "set_polish_model")]
    async fn set_polish_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
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
    /// The live subagent default model (`agent.subagents.model`), if set.
    #[method(name = "subagent_model")]
    async fn subagent_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `agent.subagents.model`; `None` clears it (children use `agent.model`). Native
    /// children spawned without `model` start on it and the system prompt names it (148 R2).
    #[method(name = "set_subagent_model")]
    async fn set_subagent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// The built-in theuth agent's model (`roster.theuth_model`), if set (148 E3 / R3):
    /// its desk and the work sessions the desk starts run on it. `None` = same as new sessions.
    #[method(name = "theuth_agent_model")]
    async fn theuth_agent_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `roster.theuth_model`; `None` clears it. An unknown or un-keyed id is refused
    /// like `set_subagent_model`. An open theuth desk switches at once.
    #[method(name = "set_theuth_agent_model")]
    async fn set_theuth_agent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
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
    /// 160 §3.2: the full-history cost cube from memory — `since` null, of
    /// another epoch or older than the change log → a full snapshot; else the
    /// hours changed since (empty when current). `CostCubeChanged` announces
    /// new revisions. The empty full cube until the engine's first build lands.
    #[method(name = "cost_cube")]
    async fn cost_cube(&self, since: Option<agent::CostCubeCursor>) -> RpcResult<agent::CostCube>;
    /// 160 §4: ledger rows newest first (native attempts and placed reported
    /// money), keyset-paged by `before`; Σ over all pages = the cube's total
    /// for the same filter.
    #[method(name = "cost_calls")]
    async fn cost_calls(&self, query: agent::CostCallsQuery) -> RpcResult<agent::CostCallsPage>;
    /// 160 §5: every provider's usage limits and theuth's budget caps, from
    /// the engine's in-memory snapshot (never a provider call).
    #[method(name = "usage_limits")]
    async fn usage_limits(&self) -> RpcResult<agent::limits::UsageLimits>;
    /// 160 §5: ask the limit pollers to refresh soon (`provider` or all); returns at once.
    #[method(name = "usage_limits_refresh")]
    async fn usage_limits_refresh(&self, provider: Option<String>) -> RpcResult<()>;
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
    /// once), `orphan` (a resumed checkout), `write` (owner unreachable ≥
    /// `sessions.takeover_secs`) or `moved` (181: the owner asked, see
    /// `session_move`). Returns the new owner/epoch;
    /// `SessionOwnerChanged` follows on the stream.
    #[method(name = "session_take", with_extensions)]
    async fn session_take(&self, session: SessionId, reason: String) -> RpcResult<SessionOwner>;
    /// 181: send `session`, owned here, to the paired engine `to` (its mesh
    /// id): `to` takes it (`session_take(moved)` over the mesh), this engine
    /// releases it, a running turn resumes there. Errors in plain words
    /// (`xeus is offline`, `this session is not owned here`, `xeus refused: …`).
    #[method(name = "session_move", with_extensions)]
    async fn session_move(&self, session: SessionId, to: String) -> RpcResult<SessionOwner>;
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
    /// A client's answer to a `FenceSnapshotRequested` (164 D3): `Started`
    /// extends the wait, the first `Done`/`Failed` resolves it. `true` while
    /// the request was pending.
    #[method(name = "answer_fence_snapshot")]
    async fn answer_fence_snapshot(
        &self,
        id: artifact::snapshot::FenceSnapshotId,
        answer: artifact::snapshot::FenceSnapshotAnswer,
    ) -> RpcResult<bool>;
    /// Every catalog model whose provider is configured. 183 W-12: `engine`
    /// (mesh id) lists that engine's catalog; `None` = this engine.
    #[method(name = "list_models", with_extensions)]
    async fn list_models(&self, engine: Option<String>) -> RpcResult<Vec<ModelInfo>>;
    /// AWS SDK-discovered profiles on this harness, plus its inference selection.
    #[method(name = "list_aws_profiles")]
    async fn list_aws_profiles(&self) -> RpcResult<AwsProfiles>;
    /// Select the harness-wide Bedrock inference profile; `None` restores the AWS default chain.
    #[method(name = "set_aws_profile")]
    async fn set_aws_profile(&self, profile: Option<String>) -> RpcResult<AwsProfiles>;
    /// With `session`: switch that session to `model` — that session only, whatever `manual`
    /// says (148 R1). With no session and `manual` (default `true`): set the default for new
    /// sessions (`agent.model`); `DefaultModelChanged` follows on the stream. With no session
    /// and `manual: false`: resolve `model` and report it, nothing else.
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
    /// Set (or clear with `None`) the reasoning effort. Values come from the model's catalog
    /// entry (`ModelInfo.efforts`, or an integer token budget within `budget_min..budget_max`).
    /// With `session`: that session from its next turn, that session only, whatever `manual`
    /// says (148 R1); `EffortChanged` follows on the stream. With no session and `manual`
    /// (default `true`): the default for new sessions (`agent.effort`); `DefaultEffortChanged`
    /// follows.
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
    /// Persist `advisor.max_consults` (0–32; 0 removes the `consult` tool) (125 R1).
    #[method(name = "set_advisor_max_consults")]
    async fn set_advisor_max_consults(&self, n: u8) -> RpcResult<AdvisorDefaults>;
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
    /// Resume a stopped ACP child in its prior harness context (75 §10, 146
    /// D8): a new harness process, `session/load` on the stored ACP session
    /// id; a still-live process for the session is killed first. A child that
    /// was mid-prompt when it died is prompted to continue (`prompted`), an
    /// idle or cancelled one is only reconnected (`idle`). Errors when the
    /// session is native, belongs to a remote node, or has no stored harness
    /// session id.
    #[method(name = "resume_child", with_extensions)]
    async fn resume_child(&self, session: SessionId) -> RpcResult<ChildResumed>;
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
    /// 183 §4.5 (W-4): on a session `enabled` pins `sessions.approvals` to
    /// `'on'`/`'off'`; `clear: true` drops the pin (inherit `tools.approvals`).
    #[method(name = "set_approvals", with_extensions)]
    async fn set_approvals(
        &self,
        session: Option<SessionId>,
        enabled: bool,
        clear: Option<bool>,
    ) -> RpcResult<ApprovalsState>;
    /// 183 §4.5 (W-5): the approvals switches as they stand, for `session`
    /// (answered by its owner) or the global default with `None`.
    #[method(name = "get_approvals", with_extensions)]
    async fn get_approvals(&self, session: Option<SessionId>) -> RpcResult<ApprovalsState>;
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
    /// 184 §4.8: "Move to ▸ machine" (owner only): the note's server writes `home = to` and hands the
    /// live copy to `to`. Notes only; boards, meeting notes, task threads and unhomed kinds are refused.
    #[method(name = "doc_move_home", with_extensions)]
    async fn doc_move_home(
        &self,
        doc: document::DocId,
        to: String,
    ) -> RpcResult<document::DocSummary>;
    /// 184 §7.2: the mesh link id (`mesh/device_key`'s public id), minted only on the lease holder
    /// (`not the hub` elsewhere). Mesh-internal — accepted only over `theuth/rpc/1` (`Forwarded`).
    #[method(name = "link_mesh_id", with_extensions)]
    async fn link_mesh_id(&self) -> RpcResult<String>;
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
    /// Save the assistant message of the turn opened at seq `turn` (`HistoryEntry.turn`, like `regenerate`) as a new
    /// note in `notebook`, chips intact (144 §8). Owner only.
    #[method(name = "save_answer_as_note", with_extensions)]
    async fn save_answer_as_note(
        &self,
        session: SessionId,
        turn: u32,
        notebook: document::DocId,
    ) -> RpcResult<agent::notebooks::SavedAnswer>;
    /// Append the assistant message of the turn opened at seq `turn` to `note` under `## From chat · <date>` (144 §8). Owner only.
    #[method(name = "promote_answer", with_extensions)]
    async fn promote_answer(
        &self,
        session: SessionId,
        turn: u32,
        note: document::DocId,
    ) -> RpcResult<()>;
    /// 144 §5.1 (wave 2): pin a notebook item's dial level for `session` (`session_context`). Owner only.
    #[method(name = "notebook_context_set", with_extensions)]
    async fn notebook_context_set(
        &self,
        session: SessionId,
        item: String,
        kind: document::ItemKind,
        level: document::ContextLevel,
    ) -> RpcResult<()>;
    /// Back to the default rule for `item` in `session`. Owner only.
    #[method(name = "notebook_context_clear", with_extensions)]
    async fn notebook_context_clear(&self, session: SessionId, item: String) -> RpcResult<()>;
    /// Pin every live item of `notebook` (of `kind` when given) at `level` for `session`. Owner only.
    #[method(name = "notebook_context_set_all", with_extensions)]
    async fn notebook_context_set_all(
        &self,
        session: SessionId,
        notebook: document::DocId,
        kind: Option<document::ItemKind>,
        level: document::ContextLevel,
    ) -> RpcResult<()>;
    /// 144 §5.2 / 157 D9: what the chat's `<notebook>` block carries — per item the effective level, pinned
    /// or not, tokens and status; the total; whether the block is already in the chat and how many change
    /// lines the next turn would send. Sized by the thread's own context, never a notebook budget. Read-only,
    /// no model call; `session: None` = a new thread (the default rule, the engine's default model). Owner only.
    #[method(name = "notebook_context_estimate", with_extensions)]
    async fn notebook_context_estimate(
        &self,
        notebook: document::DocId,
        session: Option<SessionId>,
    ) -> RpcResult<document::ContextEstimate>;
    /// 144 §4.3: one more insight section on a source — a builtin (`key_insights` | `toc` | `open_questions`)
    /// or a `notebook_insight: true` skill; answers the `###` heading's block id. Hub only; owner only.
    #[method(name = "insight_run", with_extensions)]
    async fn insight_run(
        &self,
        source: document::DocId,
        instruction: document::InsightInstruction,
    ) -> RpcResult<document::InsightRun>;
    /// Promote an insight section to a note titled `<insight> — <source>` with the provenance trailer, filed in
    /// `notebook` (or in every live notebook holding the source). Hub only; owner only.
    #[method(name = "insight_promote", with_extensions)]
    async fn insight_promote(
        &self,
        source: document::DocId,
        block: String,
        notebook: Option<document::DocId>,
    ) -> RpcResult<document::DocId>;
    /// The three builtins, then the skills whose front matter has `notebook_insight: true`.
    #[method(name = "insight_skills")]
    async fn insight_skills(&self) -> RpcResult<Vec<String>>;
    /// Ask a question across `scope` (144 §9): a new `kind: ask` session plans, searches each notebook and writes
    /// one cited answer as its first turn; the run streams as `NotebookAsk` on `subscribe_events`. `save_into` files
    /// the answer as a note there when done. Owner only.
    #[method(name = "notebook_ask", with_extensions)]
    async fn notebook_ask(
        &self,
        scope: Vec<document::DocId>,
        question: String,
        save_into: Option<document::DocId>,
    ) -> RpcResult<agent::ask::AskStarted>;
    /// The ask sessions, newest first. Owner only.
    #[method(name = "notebook_ask_list", with_extensions)]
    async fn notebook_ask_list(&self, limit: Option<u32>)
    -> RpcResult<Vec<agent::ask::AskSummary>>;
    /// Generate (or regenerate → next version) one overview of `notebook` (144 §10) in its overview session; the
    /// artifact is filed in the notebook when the `show` lands. Owner only.
    #[method(name = "notebook_overview_run", with_extensions)]
    async fn notebook_overview_run(
        &self,
        notebook: document::DocId,
        kind: agent::overviews::OverviewKind,
    ) -> RpcResult<agent::overviews::OverviewRun>;
    /// The overviews of `notebook`, newest version first per kind (a running or failed run on top). Owner only.
    #[method(name = "notebook_overviews", with_extensions)]
    async fn notebook_overviews(
        &self,
        notebook: document::DocId,
    ) -> RpcResult<Vec<agent::overviews::Overview>>;
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
    /// CodeMap overview (147 §10.2): every perspective of an indexed `root`
    /// with its element tree and `meta.state`; the first skeleton pass runs
    /// inline when the map file is missing. Store reads only, never a walk;
    /// `ASK_FAILED` when the root was never indexed.
    #[method(name = "codemap_overview")]
    async fn codemap_overview(&self, root: String) -> RpcResult<MapIndex>;
    /// CodeMap element (147 §10.2): one element — diagram, prose, edges with
    /// evidence, children with thumbnails, listing for leaves.
    #[method(name = "codemap_element")]
    async fn codemap_element(&self, root: String, element: String) -> RpcResult<MapElement>;
    /// CodeMap generate (147 §10.2, S1/S2): start the root's one generation
    /// job charged to `session` (a live native session), or attach to the
    /// running one; `element` set regenerates that subtree only.
    #[method(name = "codemap_generate")]
    async fn codemap_generate(
        &self,
        root: String,
        session: SessionId,
        element: Option<String>,
    ) -> RpcResult<MapJob>;
    /// CodeMap cancel (147 §6.7): abort the root's running job; stored
    /// elements stay.
    #[method(name = "codemap_cancel")]
    async fn codemap_cancel(&self, root: String) -> RpcResult<MapJob>;
    /// CodeMap search (147 §10.2): elements by name, or by a root-relative
    /// path → the element chain whose anchors contain it; `limit` default 20.
    #[method(name = "codemap_search")]
    async fn codemap_search(
        &self,
        root: String,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<MapHit>>;
    /// CodeMap export (147 §13.1, S3): write `<root>/.omm/` in
    /// oh-my-mermaid's layout; refuses when a foreign `.omm/` exists.
    #[method(name = "codemap_export")]
    async fn codemap_export(&self, root: String) -> RpcResult<MapExport>;
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
    /// The Connections providers: built-ins (`opencode`, `openrouter`) first, then custom providers (`/provider`).
    #[method(name = "list_providers")]
    async fn list_providers(&self) -> RpcResult<Vec<ProviderInfo>>;
    /// Persist a custom provider (url + optional key + optional local runtime preset);
    /// fetches `/models` with the context windows the server reports (177).
    /// A missing key is allowed for a local provider: its model calls carry the
    /// placeholder `Bearer local`, its listing no `Authorization` header.
    #[method(name = "add_provider")]
    async fn add_provider(
        &self,
        name: String,
        base_url: String,
        key: Option<String>,
        runtime: Option<agent::LocalRuntime>,
    ) -> RpcResult<ProviderInfo>;
    /// Forget a custom provider.
    #[method(name = "remove_provider")]
    async fn remove_provider(&self, name: String) -> RpcResult<()>;
    /// List a provider's models again (custom: `GET {base}models`; opencode/openrouter: their list) and reload the catalog (162).
    #[method(name = "refresh_provider_models")]
    async fn refresh_provider_models(&self, name: String) -> RpcResult<RefreshedModels>;
    /// 177 E8: what answers on the default local runtime ports of 127.0.0.1
    /// (11434, 1234, 8000, 8080); 1.5 s at most. Never adds anything.
    #[method(name = "local_servers_probe")]
    async fn local_servers_probe(&self) -> RpcResult<Vec<agent::LocalServer>>;
    /// 177: a provider's models with their stored limits (0 = unknown).
    #[method(name = "provider_models")]
    async fn provider_models(&self, provider: String) -> RpcResult<Vec<agent::ProviderModel>>;
    /// 177 E6: set a model's limits (source `user`; `None` clears back to unknown)
    /// and reload the catalog; running chats pick them up at their next turn.
    /// (`context` is renamed in Rust only: jsonrpsee's server glue owns that identifier.)
    #[method(name = "set_model_limits")]
    async fn set_model_limits(
        &self,
        provider: String,
        model: String,
        #[argument(rename = "context")] window: Option<u32>,
        output: Option<u32>,
    ) -> RpcResult<agent::ProviderModel>;
    /// 177 E9: one real summary (compactor) call for `model` on a fixed sample;
    /// writes no memory. Failures come back in the reply, with a hint.
    #[method(name = "test_summary_call")]
    async fn test_summary_call(&self, model: String) -> RpcResult<agent::SummaryTest>;
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
    /// The Slack app manifest this engine's Slack code expects (`slack::MANIFEST`,
    /// `crates/slack/manifest.yaml`) verbatim, for Settings › Channels › Slack ›
    /// App manifest; answers whether or not Slack is configured or enabled.
    #[method(name = "slack_manifest")]
    async fn slack_manifest(&self) -> RpcResult<String>;
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
    /// `opts` (153 §14.1): surface, destination, app profile, slash commands, press offset;
    /// `None` = today's Composer dictation.
    #[method(name = "dictation_start", with_extensions)]
    async fn dictation_start(
        &self,
        sample_rate: Option<u32>,
        opts: Option<voice::DictationOpts>,
    ) -> RpcResult<voice::DictationId>;
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
    /// 153 §6.1: the surface sent `sent_text` after dictation `id`; the engine diffs it
    /// against what it delivered and learns single-word corrections. Any authenticated peer.
    #[method(name = "dictation_correction", with_extensions)]
    async fn dictation_correction(
        &self,
        id: voice::DictationId,
        sent_text: String,
    ) -> RpcResult<()>;
    /// 153 §6.5: the newest `limit` dictation rows (raw/shaped/polished). Local peers only.
    #[method(name = "dictation_history", with_extensions)]
    async fn dictation_history(&self, limit: Option<u32>) -> RpcResult<Vec<voice::HistoryRow>>;
    /// 153 §6.7: forget one row (and its audio), or everything when `None`. Local peers only.
    #[method(name = "dictation_history_forget", with_extensions)]
    async fn dictation_history_forget(&self, id: Option<voice::DictationId>) -> RpcResult<()>;
    /// 153 §6.4: the dictation's audio (one utterance, or all) as base64 i16 16 kHz;
    /// `Err("no audio")` when evicted. Local peers only.
    #[method(name = "dictation_audio_get", with_extensions)]
    async fn dictation_audio_get(
        &self,
        id: voice::DictationId,
        utterance: Option<u32>,
    ) -> RpcResult<String>;
    /// 153 §5.6: user + learned dictionary entries, optionally one scope.
    #[method(name = "dictionary_list")]
    async fn dictionary_list(&self, scope: Option<voice::Scope>) -> RpcResult<Vec<voice::Entry>>;
    /// 153 §5.6: upsert by canonical + scope; replies with the stored entry.
    #[method(name = "dictionary_put")]
    async fn dictionary_put(&self, entry: voice::Entry) -> RpcResult<voice::Entry>;
    /// 153 §5.6: delete one entry; `false` when unknown.
    #[method(name = "dictionary_delete")]
    async fn dictionary_delete(&self, id: voice::EntryId) -> RpcResult<bool>;
    /// 153 §6.3 step 4: answer a `DictionarySuggestion` event.
    #[method(name = "dictionary_suggestion_answer")]
    async fn dictionary_suggestion_answer(
        &self,
        id: voice::SuggestionId,
        answer: voice::SuggestionAnswer,
    ) -> RpcResult<()>;
    /// 153 §8.1: every input device the host enumerates, the default flagged.
    #[method(name = "list_input_devices")]
    async fn list_input_devices(&self) -> RpcResult<Vec<voice::InputDevice>>;
    /// 153 §8.1: persist `voice.input_devices` (ordered; `[]` = system default only).
    #[method(name = "set_input_devices")]
    async fn set_input_devices(&self, names: Vec<String>) -> RpcResult<()>;
    /// 153 §4.5: persist `voice.shape.<surface>`.
    #[method(name = "set_shape")]
    async fn set_shape(&self, surface: voice::Surface, preset: voice::Preset) -> RpcResult<()>;
    /// 153 §7.2: persist `voice.end_commands`.
    #[method(name = "set_end_phrases")]
    async fn set_end_phrases(&self, phrases: voice::EndPhrases) -> RpcResult<()>;
    /// 153 F1: the live user-facing `voice.*` settings — what `set_shape`, `set_end_phrases`,
    /// `set_input_devices` and `set_polish_model` left behind, plus the config-only keys.
    #[method(name = "voice_settings")]
    async fn voice_settings(&self) -> RpcResult<voice::VoiceSettings>;
    /// 153 §9: polish `text` for a human reader at `destination`; `AgentPrompt` is refused.
    /// Fails closed: `text: None` + `refused` means deliver the shaped text.
    #[method(name = "polish")]
    async fn polish(
        &self,
        text: String,
        destination: voice::Destination,
        session: Option<SessionId>,
    ) -> RpcResult<PolishReply>;
    /// 153 §13.6: rewrite `selection` per the spoken `instruction` in `app`; `thread` carries
    /// earlier (instruction, result) pairs of the same selection.
    #[method(name = "rewrite")]
    async fn rewrite(
        &self,
        instruction: String,
        selection: String,
        app: voice::AppProfile,
        thread: Option<Vec<(String, String)>>,
    ) -> RpcResult<PolishReply>;
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
    /// Recorded meetings, newest first (`limit` default 100); 158 §9.2: `project` filters
    /// to one project.
    #[method(name = "meeting_list")]
    async fn meeting_list(
        &self,
        limit: Option<u32>,
        project: Option<String>,
    ) -> RpcResult<Vec<meeting::MeetingSummary>>;
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
    /// 156: a voiceprint from the speech of `segments` (one track): a new profile, or folded into an enrolled one.
    #[method(name = "meeting_speaker_enroll")]
    async fn meeting_speaker_enroll(
        &self,
        id: meeting::MeetingId,
        segments: Vec<u32>,
        target: meeting::EnrollTarget,
    ) -> RpcResult<meeting::Enrolled>;
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
    // 158 §15.2 — contract methods (C158). Each body is one call into the owning wave's
    // module; the stubs answer inertly until the wave lands.
    /// 158 §4.5 (W1): the current detection prompt, `null` when none.
    #[method(name = "meeting_detection")]
    async fn meeting_detection(&self) -> RpcResult<Option<meeting::Detection>>;
    /// 158 §4.4 (W1): "Not now" (10-minute cooldown) or, with `never`, "Never for <App>"
    /// (appends to `meeting.detect.ignore`).
    #[method(name = "meeting_detection_dismiss")]
    async fn meeting_detection_dismiss(&self, app_id: String, never: bool) -> RpcResult<()>;
    /// 158 §8.2 (W2): pick (or clear, `null`) the event of the current prompt; the re-rendered prompt.
    #[method(name = "meeting_detection_set_event")]
    async fn meeting_detection_set_event(
        &self,
        event: Option<meeting::EventId>,
    ) -> RpcResult<Option<meeting::Detection>>;
    /// 158 §6.2 (W1): the user-facing `meeting:` block.
    #[method(name = "meeting_settings_get")]
    async fn meeting_settings_get(&self) -> RpcResult<meeting::MeetingSettings>;
    /// 158 §6.2 (W1): persist the whole block to config.yaml and apply it from the next start;
    /// a failed write changes nothing.
    #[method(name = "meeting_settings_set")]
    async fn meeting_settings_set(
        &self,
        settings: meeting::MeetingSettings,
    ) -> RpcResult<meeting::MeetingSettings>;
    /// 158 §9.2 (W3): set or clear a meeting's project (manifest, index, sidebar, memory rows).
    #[method(name = "meeting_set_project")]
    async fn meeting_set_project(
        &self,
        id: meeting::MeetingId,
        project: Option<String>,
    ) -> RpcResult<meeting::Manifest>;
    /// 158 §8.2 (W2): set or clear a meeting's calendar event, any state; a steer while live.
    #[method(name = "meeting_set_event")]
    async fn meeting_set_event(
        &self,
        id: meeting::MeetingId,
        event: Option<meeting::EventId>,
    ) -> RpcResult<meeting::Manifest>;
    /// 158 §11.1 (W4): the note template, changeable until Summarize.
    #[method(name = "meeting_set_template")]
    async fn meeting_set_template(
        &self,
        id: meeting::MeetingId,
        template: String,
    ) -> RpcResult<meeting::Manifest>;
    /// 158 §13 (W6): Review's once-per-meeting "Did you use speakers (not headphones)?".
    #[method(name = "meeting_set_speakers")]
    async fn meeting_set_speakers(
        &self,
        id: meeting::MeetingId,
        speakers: bool,
    ) -> RpcResult<meeting::Manifest>;
    /// 158 §10 (W4): speaker-name suggestions for a reviewed meeting; never applies anything.
    #[method(name = "meeting_suggest_speakers")]
    async fn meeting_suggest_speakers(
        &self,
        id: meeting::MeetingId,
    ) -> RpcResult<Vec<meeting::Suggestion>>;
    /// 158 §11.1 (W4): every template, built-ins first.
    #[method(name = "meeting_templates")]
    async fn meeting_templates(&self) -> RpcResult<Vec<meeting::Template>>;
    /// 158 §11.1 (W4): write `<slug>.md` (markdown + YAML front matter) and return it parsed.
    #[method(name = "meeting_template_put")]
    async fn meeting_template_put(
        &self,
        slug: String,
        body: String,
    ) -> RpcResult<meeting::Template>;
    /// 158 §11.1 (W4): delete a user template; refused for built-ins.
    #[method(name = "meeting_template_delete")]
    async fn meeting_template_delete(&self, slug: String) -> RpcResult<()>;
    /// 158 §11.1 (W4): restore a built-in's shipped body over the user's edits; refused for user templates.
    #[method(name = "meeting_template_reset")]
    async fn meeting_template_reset(&self, slug: String) -> RpcResult<meeting::Template>;
    /// 158 §7.4 (W2): every calendar source.
    #[method(name = "calendar_sources")]
    async fn calendar_sources(&self) -> RpcResult<Vec<meeting::CalSource>>;
    /// 158 §7.3 (W2): add an ICS subscription; the URL goes to the vault as `calendar.ics.<id>`.
    #[method(name = "calendar_source_add")]
    async fn calendar_source_add(&self, name: String, url: String)
    -> RpcResult<meeting::CalSource>;
    /// 158 §7.4 (W2): delete a source and its events (and vault row).
    #[method(name = "calendar_source_delete")]
    async fn calendar_source_delete(&self, id: String) -> RpcResult<()>;
    /// 158 §7.3 (W2): fetch now; the row with `last_fetch` / `last_error`.
    #[method(name = "calendar_source_refresh")]
    async fn calendar_source_refresh(&self, id: String) -> RpcResult<meeting::CalSource>;
    /// 158 §7.2 (W2), companion only (`x-theuth-client: companion`): replace `source`'s rows
    /// inside `window` (RFC 3339 pair) with `events`; the rows written.
    #[method(name = "calendar_push", with_extensions)]
    async fn calendar_push(
        &self,
        source: String,
        window: (String, String),
        events: Vec<meeting::CalEvent>,
    ) -> RpcResult<u32>;
    /// 158 §7.2 (W2), companion only: the EventKit state line (`granted` / `denied` / …).
    #[method(name = "calendar_source_state", with_extensions)]
    async fn calendar_source_state(&self, source: String, state: String) -> RpcResult<()>;
    /// 158 §7.4 (W2): events overlapping `[from, to]` (RFC 3339), start ascending.
    #[method(name = "calendar_events")]
    async fn calendar_events(&self, from: String, to: String) -> RpcResult<Vec<meeting::CalEvent>>;
    /// 158 §7.6 (W2): the `meeting.calendar` block.
    #[method(name = "calendar_settings_get")]
    async fn calendar_settings_get(&self) -> RpcResult<meeting::CalendarConfig>;
    /// 158 §7.6 (W2): persist and apply the `meeting.calendar` block.
    #[method(name = "calendar_settings_set")]
    async fn calendar_settings_set(
        &self,
        config: meeting::CalendarConfig,
    ) -> RpcResult<meeting::CalendarConfig>;
    /// 158 §12.6 (W5): the MCP server's config, whether it is listening, URL and token.
    #[method(name = "mcp_server_get")]
    async fn mcp_server_get(&self) -> RpcResult<mcp_server::McpServerStatus>;
    /// 158 §12.2 (W5): persist `mcp_server` and bind or drop the listener live.
    #[method(name = "mcp_server_set")]
    async fn mcp_server_set(
        &self,
        config: mcp_server::McpServerConfig,
    ) -> RpcResult<mcp_server::McpServerStatus>;
    /// 158 §12.2 (W5): a fresh token into the vault; every open MCP session ends.
    #[method(name = "mcp_server_rotate_token")]
    async fn mcp_server_rotate_token(&self) -> RpcResult<()>;
    /// 158 §12.5 (W5): the last `limit` audit lines, newest first.
    #[method(name = "mcp_server_audit")]
    async fn mcp_server_audit(&self, limit: u32) -> RpcResult<Vec<mcp_server::AuditLine>>;
    /// 158 §12.3 (W5): one catalogue tool call for the `t mcp` stdio bridge, run in the engine
    /// with the vault's redaction and an audit line `transport: stdio`; `client` is the MCP
    /// `clientInfo.name`. Unknown tool / bad arguments → invalid params.
    #[method(name = "mcp_server_call")]
    async fn mcp_server_call(
        &self,
        tool: String,
        args: serde_json::Value,
        client: String,
    ) -> RpcResult<mcp_server::McpCallResult>;
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
    /// 185 A5: on a peered non-hub with the hub connected the press runs on the hub (forwarded).
    #[method(name = "dreaming.run_now", with_extensions)]
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
    /// 140 §12.5: every study, newest first.
    #[method(name = "autoresearch.list")]
    async fn autoresearch_list(&self) -> RpcResult<Vec<autoresearch::StudyInfo>>;
    /// 140 §12.5: one study with its spec, incumbent, σ, budget and chart series.
    #[method(name = "autoresearch.get")]
    async fn autoresearch_get(&self, slug: String) -> RpcResult<autoresearch::StudyDetail>;
    /// 140 §12.5: run-table rows newest first; `before` pages.
    #[method(name = "autoresearch.experiments")]
    async fn autoresearch_experiments(
        &self,
        slug: String,
        before: Option<u32>,
        limit: u32,
    ) -> RpcResult<Vec<autoresearch::ExperimentRow>>;
    /// 140 §12.5: one experiment — samples, verdict line, deviations, review, diff, log tails.
    #[method(name = "autoresearch.experiment")]
    async fn autoresearch_experiment(
        &self,
        slug: String,
        n: u32,
    ) -> RpcResult<autoresearch::ExperimentDetail>;
    /// 140 §12.5 / §8.6: pause, resume, stop_after, stop_now, run_once, segment, finalize, restore, discard.
    #[method(name = "autoresearch.control")]
    async fn autoresearch_control(
        &self,
        slug: String,
        op: autoresearch::ControlOp,
    ) -> RpcResult<autoresearch::StudyInfo>;
    /// 140 §7.3: a steer reaches the next brief without pausing; returns the row id.
    #[method(name = "autoresearch.steer")]
    async fn autoresearch_steer(&self, slug: String, text: String) -> RpcResult<i64>;
    /// 140 §7.3: add, reprioritise, close or reopen a backlog idea.
    #[method(name = "autoresearch.idea")]
    async fn autoresearch_idea(
        &self,
        slug: String,
        op: autoresearch::IdeaChange,
    ) -> RpcResult<autoresearch::Idea>;
    /// 140 §12.4: the draft card's Start press — creates the study from the draft.
    #[method(name = "autoresearch.create")]
    async fn autoresearch_create(
        &self,
        draft_id: String,
        start: autoresearch::StartWhen,
    ) -> RpcResult<autoresearch::StudyInfo>;
    /// Settings › Autoresearch as running (140 §12.2); from the config section even when disabled.
    #[method(name = "autoresearch.settings.get")]
    async fn autoresearch_settings_get(&self) -> RpcResult<autoresearch::AutoresearchConfig>;
    /// Persist the `autoresearch:` section and apply it live (140 §12.5).
    #[method(name = "autoresearch.settings.set")]
    async fn autoresearch_settings_set(
        &self,
        config: autoresearch::AutoresearchConfig,
    ) -> RpcResult<autoresearch::AutoresearchConfig>;
    /// 151 §4.4: the live draft panel — rows with engine-computed states, `ready`, what blocks Start.
    #[method(name = "autoresearch.draft")]
    async fn autoresearch_draft(&self, draft_id: String) -> RpcResult<autoresearch::DraftView>;
    /// 151 D6: drafts with no study yet (the rail's "Setting up" rows), newest first.
    #[method(name = "autoresearch.drafts")]
    async fn autoresearch_drafts(&self) -> RpcResult<Vec<autoresearch::DraftSummary>>;
    /// 151 D3: a leaf set in the panel (`metric.name`, `scope`, …; text, parsed by the engine) — user-set and confirmed.
    #[method(name = "autoresearch.draft_edit")]
    async fn autoresearch_draft_edit(
        &self,
        draft_id: String,
        field: String,
        value: String,
    ) -> RpcResult<autoresearch::DraftView>;
    /// 151 D3: a row OK'd in the panel (`proposed` → `confirmed`).
    #[method(name = "autoresearch.draft_confirm")]
    async fn autoresearch_draft_confirm(
        &self,
        draft_id: String,
        field: String,
    ) -> RpcResult<autoresearch::DraftView>;
    /// 151 D3: discard a draft — the drafts dir is removed, later Starts are refused.
    #[method(name = "autoresearch.discard_draft")]
    async fn autoresearch_discard_draft(&self, draft_id: String) -> RpcResult<()>;
    /// 159 §5.11: archive rows — front members (W1) or population cells (W4).
    #[method(name = "autoresearch.archive")]
    async fn autoresearch_archive(&self, slug: String) -> RpcResult<Vec<autoresearch::ArchiveRow>>;
    /// 159 §5.11: case × experiment rows for the heat strip; `n` narrows to one experiment (W2).
    #[method(name = "autoresearch.cases")]
    async fn autoresearch_cases(
        &self,
        slug: String,
        n: Option<u32>,
    ) -> RpcResult<Vec<autoresearch::CaseRow>>;
    /// 159 §5.11: blind-pair verdicts; `n` narrows to one experiment (W3).
    #[method(name = "autoresearch.judgments")]
    async fn autoresearch_judgments(
        &self,
        slug: String,
        n: Option<u32>,
    ) -> RpcResult<Vec<autoresearch::JudgmentRow>>;
    /// 159 §5.11: the user's answer (`a` | `b` | `same`) to blind pair `id` (W3).
    #[method(name = "autoresearch.human_pair")]
    async fn autoresearch_human_pair(
        &self,
        slug: String,
        id: i64,
        winner: String,
    ) -> RpcResult<autoresearch::StudyInfo>;
    /// 159 §5.11: the launch press — approve deploys both arms, refuse discards experiment `n` (W6).
    #[method(name = "autoresearch.launch")]
    async fn autoresearch_launch(
        &self,
        slug: String,
        n: u32,
        token: String,
        approve: bool,
    ) -> RpcResult<autoresearch::StudyInfo>;
    /// 159 §5.11: list, show or adopt a mechanism version (W7).
    #[method(name = "autoresearch.mechanism")]
    async fn autoresearch_mechanism(
        &self,
        op: autoresearch::MechanismOp,
    ) -> RpcResult<autoresearch::MechanismInfo>;
    // 171:C cells (design 171 §15.2). Bodies: `crate::cells` (E1), `crate::cells_latest` (E2).
    /// 171: run one cell. Press guard (§12.1 inv. 1) + `authorize`; idempotent per `run`.
    /// `confirmed: true` is the user's second press past a warning (§7.1, §7.6): Run anyway
    /// past an unarmed `after` gate, or a danger cell's line/selection ►; omitted or `false`,
    /// both are refused as before.
    #[method(name = "cell_run", with_extensions)]
    async fn cell_run(
        &self,
        cell: cells::CellRef,
        seen: String,
        mode: cells::RunMode,
        run: cells::RunId,
        edited: Option<String>,
        member_ack: Option<String>,
        confirmed: Option<bool>,
    ) -> RpcResult<cells::CellRun>;
    /// 171: Run all as an engine pass (§7.2); returns the pass id.
    #[method(name = "cell_run_all", with_extensions)]
    async fn cell_run_all(
        &self,
        scope: cells::Scope,
        cells: Vec<(cells::CellRef, String)>,
        pass: String,
    ) -> RpcResult<String>;
    /// 171: Stop (`kill: false`) or Kill a run; a queued run is cancelled.
    #[method(name = "cell_stop", with_extensions)]
    async fn cell_stop(&self, run: cells::RunId, kill: bool) -> RpcResult<()>;
    /// 171: bytes (base64) typed into a run's live terminal / its `input()` answer.
    #[method(name = "cell_input", with_extensions)]
    async fn cell_input(&self, run: cells::RunId, data: String) -> RpcResult<()>;
    /// 171: Ask theuth — wake the session with that run's result.
    #[method(name = "cell_ask", with_extensions)]
    async fn cell_ask(&self, run: cells::RunId) -> RpcResult<()>;
    /// 171: a run's live terminal from mark C (`reset`, `data`…, `exit`).
    #[subscription(name = "cell_attach", unsubscribe = "cell_detach", item = PtyChunk, with_extensions)]
    async fn cell_attach(&self, run: cells::RunId) -> SubscriptionResult;
    /// 171: problems, chips, armed/superseded/stale per cell and the scope's kernels.
    #[method(name = "cell_state", with_extensions)]
    async fn cell_state(
        &self,
        scope: cells::Scope,
        cells: Vec<cells::CellRef>,
    ) -> RpcResult<cells::StateReply>;
    /// 171: one run's record and outputs (text capped).
    #[method(name = "cell_run_get", with_extensions)]
    async fn cell_run_get(&self, run: cells::RunId) -> RpcResult<cells::RunGet>;
    /// 171: a page of a run's cleaned (or `ansi`) text.
    #[method(name = "cell_output", with_extensions)]
    async fn cell_output(
        &self,
        run: cells::RunId,
        offset: u64,
        limit: u32,
        ansi: bool,
    ) -> RpcResult<cells::OutputPage>;
    /// 171: one file of a run (images), base64.
    #[method(name = "cell_output_file", with_extensions)]
    async fn cell_output_file(&self, run: cells::RunId, name: String) -> RpcResult<String>;
    /// 171: a scope's runs, newest first.
    #[method(name = "cell_runs", with_extensions)]
    async fn cell_runs(&self, scope: cells::Scope, limit: u32) -> RpcResult<Vec<cells::CellRun>>;
    /// 171: interpreter candidates for `cwd` on this engine or the named one (§6.4).
    #[method(name = "cell_interpreters", with_extensions)]
    async fn cell_interpreters(
        &self,
        cwd: String,
        engine: Option<String>,
    ) -> RpcResult<Vec<cells::Interpreter>>;
    /// 171: Restart a kernel (Close; the next run starts a fresh one).
    #[method(name = "cell_kernel_restart", with_extensions)]
    async fn cell_kernel_restart(&self, kernel: cells::KernelId) -> RpcResult<()>;
    /// 171: Close a kernel.
    #[method(name = "cell_kernel_close", with_extensions)]
    async fn cell_kernel_close(&self, kernel: cells::KernelId) -> RpcResult<()>;
    /// 171: re-capture the login environment.
    #[method(name = "cell_env_refresh", with_extensions)]
    async fn cell_env_refresh(&self) -> RpcResult<()>;
    /// 171: runs in flight on this engine (quit / `t update` sheet).
    #[method(name = "cell_live", with_extensions)]
    async fn cell_live(&self) -> RpcResult<Vec<cells::CellRun>>;
    /// 171: a note's replicated latest runs (§13.2); owner, member or guest.
    #[method(name = "cell_latest", with_extensions)]
    async fn cell_latest(&self, doc: document::DocId) -> RpcResult<Vec<cells::CellLatest>>;
    /// 171F B9: the note a chat reply was opened as, if any; owner, not guest.
    #[method(name = "cell_reply_note_get", with_extensions)]
    async fn cell_reply_note_get(
        &self,
        session: cells::SessionId,
        entry: String,
    ) -> RpcResult<Option<cells::DocId>>;
    /// 171F B9: remember that reply `entry` of `session` was opened as note `doc`.
    #[method(name = "cell_reply_note_set", with_extensions)]
    async fn cell_reply_note_set(
        &self,
        session: cells::SessionId,
        entry: String,
        doc: cells::DocId,
    ) -> RpcResult<()>;
    /// 137 §11.1 `agent.endless` as in force (Settings › Memory).
    #[method(name = "endless_settings_get")]
    async fn endless_settings_get(&self) -> RpcResult<EndlessConfig>;
    /// Persist the whole `agent.endless` block to config.yaml and apply it
    /// live (defaults for new chats/desks/meetings, view budget, slack,
    /// settle). A changed compactor model applies to every attached session
    /// at once (137F). A failed write changes nothing.
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
    /// Leave an engine out of the mesh (67 §4.1); 183 D7: the remaining machines then rotate
    /// the vault key. `reset_devices` (D11, default false): also rotate `mesh/device_key`
    /// after the vault key, so every phone and iPad re-pairs.
    #[method(name = "mesh_remove")]
    async fn mesh_remove(&self, id: String, reset_devices: Option<bool>) -> RpcResult<()>;
    /// This engine's hub lease preference (67 §4.5); written to `mesh.hub`.
    #[method(name = "mesh_set_hub")]
    async fn mesh_set_hub(&self, pref: mesh::HubPref) -> RpcResult<()>;
    /// 183 §6.8: engines the Nearby browse sees, not paired; empty while no window is open.
    #[method(name = "mesh_nearby")]
    async fn mesh_nearby(&self) -> RpcResult<Vec<mesh::NearbyEngine>>;
    /// 183 §6.8: open (`true`, Settings › Network shown) or close the Nearby window. `true` is a
    /// 30 s lease: the open page re-sends it every 10 s and on reconnect.
    #[method(name = "mesh_nearby_watch")]
    async fn mesh_nearby_watch(&self, open: bool) -> RpcResult<()>;
    /// 183 §6.8: start a code-checked pairing with Nearby engine `id`.
    #[method(name = "mesh_pair_nearby")]
    async fn mesh_pair_nearby(&self, id: String) -> RpcResult<mesh::PairCeremony>;
    /// 183 §6.8: this side's Pair (`accept`) or Cancel for a ceremony.
    #[method(name = "mesh_pair_answer")]
    async fn mesh_pair_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()>;
    /// 183 §6.2 (W-14): agents on this engine and `id` may message each other (`on`), or
    /// not; written to `mesh.messaging_off`.
    #[method(name = "mesh_set_messaging")]
    async fn mesh_set_messaging(&self, id: String, on: bool) -> RpcResult<()>;
    /// 183 R1 (S1): keep (`keep`: pair it again) or drop a machine an unpaired machine
    /// introduced (`MeshStatus.quarantined`).
    #[method(name = "mesh_confirm_peer")]
    async fn mesh_confirm_peer(&self, id: String, keep: bool) -> RpcResult<()>;
    /// 183 §6.7: "Let nearby machines find this one while pairing"; written to `mesh.nearby`.
    #[method(name = "mesh_set_nearby")]
    async fn mesh_set_nearby(&self, on: bool) -> RpcResult<()>;
    /// 183 §6.7: rename a machine: `mesh.name` for this engine, `peers.name` otherwise.
    #[method(name = "mesh_rename")]
    async fn mesh_rename(&self, id: String, name: String) -> RpcResult<()>;
    /// 183 §12.9: what unpairing `id` means, for the Unpair dialog.
    #[method(name = "mesh_unpair_preview")]
    async fn mesh_unpair_preview(&self, id: String) -> RpcResult<mesh::UnpairPreview>;
    /// 183 §6.9: the newest vault-key rotation; `null` before the first.
    #[method(name = "vault_rotation")]
    async fn vault_rotation(&self) -> RpcResult<Option<secrets::RotationState>>;
    /// 183 §6.9.12: rotate the vault key without an unpair, or retry a failed rotation.
    #[method(name = "vault_rotate")]
    async fn vault_rotate(&self) -> RpcResult<secrets::RotationState>;
    /// 183 §7.4: open (`true`, the Add contact sheet; returns the invite ticket) or close the
    /// Add contact window.
    #[method(name = "contact_window")]
    async fn contact_window(&self, open: bool) -> RpcResult<Option<a2a::contacts::ContactWindow>>;
    /// 183 §7.1: A2A agents the browse sees while the window is open, newest first, ≤ 32.
    #[method(name = "contact_nearby")]
    async fn contact_nearby(&self) -> RpcResult<Vec<a2a::contacts::NearbyAgent>>;
    /// 183 §7.4: start the ceremony with a Nearby agent, a ticket or a card URL.
    #[method(name = "contact_pair")]
    async fn contact_pair(
        &self,
        target: a2a::contacts::ContactTarget,
    ) -> RpcResult<mesh::PairCeremony>;
    /// 183 §7.4: this side's Accept (`accept`) or Reject for a contact ceremony.
    #[method(name = "contact_answer")]
    async fn contact_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()>;
    /// 183 §12.7: this machine's contacts.
    #[method(name = "contact_list")]
    async fn contact_list(&self) -> RpcResult<Vec<a2a::contacts::Contact>>;
    /// 183 §12.7: rename, cap, grant exports, block or unblock a contact.
    #[method(name = "contact_update")]
    async fn contact_update(
        &self,
        name: String,
        patch: a2a::contacts::ContactPatch,
    ) -> RpcResult<a2a::contacts::Contact>;
    /// 183 §8.4: Remove (`block: false`) or Block a contact: local and immediate.
    #[method(name = "contact_remove")]
    async fn contact_remove(&self, name: String, block: bool) -> RpcResult<()>;
    /// 183 §7.6, D9: this machine's exports (agents and chats), with `paused`.
    #[method(name = "a2a_exports")]
    async fn a2a_exports(&self) -> RpcResult<Vec<a2a::contacts::Export>>;
    /// 183 §7.6: create or replace an export by name.
    #[method(name = "a2a_export_put")]
    async fn a2a_export_put(&self, export: a2a::contacts::Export) -> RpcResult<()>;
    /// 183 §12.10: remove an export and its grants (Stop sharing).
    #[method(name = "a2a_export_remove")]
    async fn a2a_export_remove(&self, name: String) -> RpcResult<()>;
    /// 183 Z4: the tools an export may allow a contact (the allowlist editor offers only these).
    #[method(name = "a2a_grantable_tools")]
    async fn a2a_grantable_tools(&self) -> RpcResult<Vec<tools::GrantableTool>>;
    /// 183 §7.6 (W-27): the `contact_first` Attention item's Allow once · Always · Block.
    #[method(name = "contact_first_answer")]
    async fn contact_first_answer(
        &self,
        contact: String,
        export: String,
        answer: a2a::config::FirstContact,
    ) -> RpcResult<()>;
    /// 183 F3, F11: every (contact, export) whose first message waits for the owner's
    /// answer — the source of the `contact_first` Attention item; refetch on `A2aChanged`.
    #[method(name = "contact_first_waiting")]
    async fn contact_first_waiting(&self) -> RpcResult<Vec<a2a::contacts::FirstWaiting>>;
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
    /// 182 D10: the task lives on engine `to` (a mesh id) from now on — only its desk works it there.
    /// `to` must be online and have the task's agent (theuth is everywhere); a done or cancelled
    /// task does not move. A live work session of its checkout owned here moves first
    /// (`session_move`); that move's refusal refuses this one. Off an offline home the move
    /// lands here at once and replicates when that engine returns.
    #[method(name = "task_move_home", with_extensions)]
    async fn task_move_home(&self, id: String, to: String) -> RpcResult<tasks::Task>;
    /// 182 D9 "Move them to <this engine>": every task waiting on the offline engine `from`
    /// moves to this engine; the ones whose agent is not here are named in `refused`.
    #[method(name = "task_move_home_waiting", with_extensions)]
    async fn task_move_home_waiting(&self, from: String) -> RpcResult<tasks::MovedHome>;
    /// 182 D8: every engine's agents (the replicated per-engine roster, this engine's rows
    /// included), by name then engine — the Agents page rows and the Move to checks.
    #[method(name = "engine_agents_list")]
    async fn engine_agents_list(&self) -> RpcResult<Vec<roster::EngineAgent>>;
    /// The activity feed, newest first: `since_ms`, `kinds`, `limit` (default 50, max 500) (70 HUD).
    #[method(name = "activity_list")]
    async fn activity_list(
        &self,
        filter: tasks::activity::ActivityFilter,
    ) -> RpcResult<Vec<tasks::activity::ActivityEntry>>;
    /// The attention inbox (70 HUD): approvals, decisions, hires, blocked / review tasks,
    /// budgets, unreachable nodes — undismissed and unsnoozed, newest first. Badge = length.
    /// 155 §7.3 / §7.5: per connection — `alert` is blanked for items whose
    /// session this connection focuses (`set_event_focus`) and for `finished`
    /// alerts it has already seen.
    #[method(name = "attention_list", with_extensions)]
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
    /// overwrite); returns the written path (35 §3.5). A private artifact
    /// needs its owning `session` (authorized, 51 §4.2).
    #[method(name = "export_artifact", with_extensions)]
    async fn export_artifact(
        &self,
        id: String,
        dest: PathBuf,
        session: Option<SessionId>,
    ) -> RpcResult<PathBuf>;
    /// A committed app record: its artifact, source, compatibility hash,
    /// pinned tool bindings and diagnostics (46 §6.3, §8.3). Rejects with
    /// `-32001` when the app plugin is disabled: the client falls back to
    /// the artifact's readable source. A private record needs its owning
    /// `session` (authorized, 51 §4.2).
    #[method(name = "get_app", with_extensions)]
    async fn get_app(
        &self,
        record_call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<AppDoc>;
    /// Publish a reply fence's OpenUI source as an app artifact of `session` without a
    /// model turn (30 §9): validated and committed exactly like the `app` tool with
    /// `edit: false`, under a synthetic `ui-<uuid v7>` call id, no transcript entry. The
    /// error message is the tool's located diagnostics text. `session` is
    /// authorized: a private tree takes apps from its owner only.
    #[method(name = "publish_app", with_extensions)]
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
    /// A private record is served only to its tree's owner naming the
    /// `session` (authorized, 51 §4.2), as `get_attachment`.
    #[method(name = "get_artifact", with_extensions)]
    async fn get_artifact(
        &self,
        call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<artifact::Artifact>;
    /// The member listing of a `Kind::Archive` record (35 §3.2): paths,
    /// sizes and dates from the archive's headers, at most 5,000 entries
    /// (`truncated` past that). Rejects when the record vanished or the
    /// archive was unreadable at show time (no listing sidecar). A private
    /// record needs its owning `session`, as `get_artifact`.
    #[method(name = "get_artifact_listing", with_extensions)]
    async fn get_artifact_listing(
        &self,
        call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<artifact::archive::ArchiveListing>;
    /// Up to 2,000 characters of the artifact's indexed text around the
    /// occurrence `query` matches, with highlight ranges and content coverage;
    /// unavailable extraction is an error, never the live `origin` (49 §7).
    /// Durable records only: a private tree is never indexed (51 §4.5).
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
    /// 176 C1: liveness only — answers `null` at once. Asks no actor and awaits nothing, so a
    /// busy engine still answers; probe with this, not `info`, to tell a dead socket from a
    /// slow handler.
    #[method(name = "ping")]
    fn ping(&self) -> RpcResult<()>;
    /// 176 C2: one client diagnostic line into the engine log, target `client`, fields
    /// `source` and `conn` (the `rpc: ws open conn=<n>` id; 0 off the local WebSocket).
    /// `level` `"warn"` logs at WARN, anything else at INFO; `source` is ≤32 chars of
    /// `[a-z0-9-]` (else logged as `-`); `message` is cut to 2,000 chars. 20 per minute per
    /// connection; the rest of that minute is dropped with one WARN. Asks no actor.
    #[method(name = "client_log", with_extensions)]
    fn client_log(&self, level: String, source: String, message: String) -> RpcResult<()>;
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
        // 183 W-19: cards relayed from the owners of followed sessions, so this
        // snapshot (and the one after `Lagged`) keeps them.
        let relayed = sessionsync::events::relayed_prompts();
        out.approvals.extend(admit(relayed.approvals, &mut keep));
        out.asks.extend(admit(relayed.asks, &mut keep));
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

/// 155 §7.5: `connection id → finished alert ids` this connection has seen
/// (it focused the item's session after the finish). Dropped with the
/// connection like [`FocusMap`]; `attention_list` blanks seen `finished`
/// alerts for that connection only.
pub type SeenMap = Arc<RwLock<HashMap<u64, HashSet<String>>>>;

/// 183 §5.1: `connection id → focused sessions` with the relay's follow guard
/// held for each (a session owned elsewhere streams while focused). Dropped
/// with the focus and with the connection.
pub type FollowMap =
    Arc<parking_lot::Mutex<HashMap<u64, HashMap<SessionId, sessionsync::events::Following>>>>;

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
    /// Per-connection finished-seen set; see [`SeenMap`].
    pub seen: SeenMap,
    /// Per-connection follow guards of focused sessions; see [`FollowMap`].
    pub follows: FollowMap,
}

impl Server {
    /// A server with no event feed: for in-process callers (the TUI), which
    /// subscribe to the bus themselves. `serve` builds the real one.
    pub fn new(rt: Runtime) -> Self {
        install_insight_hooks(&rt);
        Self {
            events: broadcast::channel(1).0,
            rt,
            private: PRIVATE.clone(),
            focus: Default::default(),
            seen: Default::default(),
            follows: Default::default(),
        }
    }

    /// 129 §3 / 155 §7.5 / 183 §5.1: drop the connection's focus, seen and
    /// follow entries when it closes. A second waiter for the same peer (a
    /// map was fresh) removes nothing new; one spawned after the close
    /// removes at once.
    fn forget_on_close(&self, peer: link::RpcPeer) {
        let (focus, seen, follows) = (self.focus.clone(), self.seen.clone(), self.follows.clone());
        tokio::spawn(async move {
            peer.closed().await;
            focus.write().remove(&peer.id());
            seen.write().remove(&peer.id());
            follows.lock().remove(&peer.id());
        });
    }

    /// The `double:` section as read at start (126 §12); defaults when absent.
    fn double_config(&self) -> double::DoubleConfig {
        self.rt.section("double").unwrap_or_default()
    }

    /// The `dreaming:` section as read at start (136 §9.2); defaults when absent.
    fn dreaming_config(&self) -> dreaming::DreamConfig {
        self.rt.section("dreaming").unwrap_or_default()
    }

    /// The `autoresearch:` section as read at start (140 §12.2); defaults when absent.
    fn autoresearch_config(&self) -> autoresearch::AutoresearchConfig {
        self.rt.section("autoresearch").unwrap_or_default()
    }

    /// 153 §14.3: persist `voice.<key>` (one or two levels; the value is YAML flow syntax).
    fn write_voice_config(&self, key: &[&str], value: &str) -> RpcResult<()> {
        let path = self.rt.data_dir.join("config.yaml");
        match key {
            [k] => runtime::write_config_value(&path, "voice", k, value),
            [sub, k] => runtime::write_nested_config_value(&path, "voice", sub, k, value),
            _ => unreachable!("one or two key levels"),
        }
        .map_err(|e| failed(format!("config.yaml: {e}")))
    }
}

impl Server {
    /// The one admission check (51 §4.2): the calling connection may act on
    /// `session`. Durable sessions admit any authenticated peer; a private
    /// one only its owner; unknown or closing ids fail closed.
    pub(crate) async fn authorize(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Authorized> {
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
    pub(crate) async fn authorize_opt(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<()> {
        if let Some(s) = session {
            self.authorize(ext, s).await?;
        }
        Ok(())
    }

    /// [`Self::authorize`] for a handler that forwards (`forward::route`,
    /// `forward::to_creator`): 183 §5.6, a chat another engine just created
    /// for us whose row is not here yet is admitted (durable chats only, see
    /// `create_session`), since the call goes to that engine, which admits
    /// it. Every other handler keeps the strict check and fails closed.
    pub(crate) async fn authorize_fwd(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<()> {
        peer(ext)?;
        if let Some(forward::Expected { here: false, .. }) = forward::expected(ext, session).await?
        {
            return Ok(());
        }
        self.authorize(ext, session).await.map(drop)
    }

    /// [`Self::authorize_fwd`] when a session is named.
    pub(crate) async fn authorize_opt_fwd(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<()> {
        if let Some(s) = session {
            self.authorize_fwd(ext, s).await?;
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

    /// 183 §5.6: wait, at most [`CREATED_ROW_WAIT`], for a session `engine`
    /// just created to replicate here (§13 row 11: on timeout the caller
    /// answers anyway; calls for it are forwarded by id meanwhile).
    async fn wait_for_row(&self, session: SessionId, engine: &str) {
        let Ok(store) = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await else {
            return;
        };
        let deadline = Instant::now() + CREATED_ROW_WAIT;
        while Instant::now() < deadline {
            if store.session(session).await.ok().flatten().is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        tracing::warn!(
            "sessions: new row {session} from {engine} not here after {}s; forwarding by id",
            CREATED_ROW_WAIT.as_secs()
        );
    }
}

/// 183 §5.6: how long `create_session{engine}` waits for the new row before it
/// answers (the chat works by id meanwhile; the wait only spares a lookup).
const CREATED_ROW_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

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

/// A `dreaming::rpc` / `autoresearch::rpc_impl` error → JSON-RPC: "not running" is `NOT_RUNNING`, anything else `ASK_FAILED`.
fn dreaming_err(e: String) -> ErrorObjectOwned {
    let code = if e.ends_with("is not running") {
        NOT_RUNNING
    } else {
        ASK_FAILED
    };
    ErrorObjectOwned::owned(code, e, None::<()>)
}

/// 185 R1 A5: the hub's refusal of a forwarded Dream now, said as the hub's —
/// "Dream now runs on the hub, which …" — with the code and data kept (a
/// `-32010` still carries `OwnerUnreachable`). Nothing runs here instead: the
/// hub may have started the night.
fn hub_said(e: ErrorObjectOwned) -> ErrorObjectOwned {
    let msg = if e.code() == forward::OWNER_UNREACHABLE {
        format!(
            "Dream now runs on the hub, which did not answer ({}); try again when it is back",
            e.message()
        )
    } else {
        format!("Dream now runs on the hub, which said: {}", e.message())
    };
    ErrorObjectOwned::owned(e.code(), msg, e.data().map(|d| d.to_owned()))
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
/// engine finds the actor holding the oneshot; 183 W-19: a card relayed from a
/// followed owner before its row replicated names its session too. `None`: no
/// row (a pre-0033 question or a non-journaled kind), or the session is
/// unknown / ours.
async fn question_owner(
    store: &agent::store::Store,
    id: &Uuid,
) -> RpcResult<Option<replica::PeerId>> {
    let session = match store
        .question_get(&id.to_string())
        .await
        .map_err(|e| failed(format!("{e:#}")))?
    {
        Some(row) => row.session,
        None => match sessionsync::events::relayed_session(*id) {
            Some(s) => s,
            None => return Ok(None),
        },
    };
    let Some((owner, _)) = store
        .owner(session)
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

/// The 153 history store (§6.5): local peers only, like the daemon mic
/// (`dictation_start`); `ASK_FAILED` "history disabled" when it is off.
async fn history_store(ext: &Extensions) -> RpcResult<voice::Store> {
    if peer(ext)?.remote() {
        return Err(failed("dictation history is on the harness host"));
    }
    send(&dictation()?, voice::HistoryStore)
        .await?
        .ok_or_else(|| failed("history disabled"))
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
pub(crate) fn peer(ext: &Extensions) -> RpcResult<link::RpcPeer> {
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

/// 184 §4.8, §7.2: an owner call on `doc` re-issued to the engine that serves it (`DocServer`), so
/// that engine checks its own state (its Link, the home). `Ok(None)`: run it here — this engine
/// serves it or nobody does, the server is not connected, or this call was itself forwarded.
async fn to_doc_server<T: DeserializeOwned>(
    ext: &Extensions,
    docs: &ActorRef<document::DocumentActor>,
    doc: &document::DocId,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    let Ok(Some(server)) = doc_send(docs, document::DocServer { doc: doc.clone() }).await else {
        return Ok(None);
    };
    forward::to_peer(ext, &replica::PeerId(server.0), method, params).await
}

/// `doc_join`'s outcome on the guest's conn (59 §8.3): success scopes it; only a refusal counts
/// toward its close — the join's own (`JoinError`'s wire codes) and a member call's (Z184-R2/R3):
/// a retryable `busy` (184 §4.3 step 4), a `hub_moved` during a Move or any transient error never
/// locks a guest out.
fn join_outcome<T>(guest: &link::Guest, r: &RpcResult<T>) {
    const REFUSALS: [&str; 10] = [
        "not_member",
        "viewer_readonly",
        "sharing_disabled",
        "bad_proof",
        "link_unknown",
        "link_revoked",
        "link_expired",
        "members_full",
        "sharing_off",
        "member_revoked",
    ];
    match r {
        Ok(_) => guest.mark_joined(),
        Err(e) if REFUSALS.iter().any(|r| e.message().starts_with(r)) => {
            guest.refuse();
        }
        Err(_) => {}
    }
}

/// `doc_link_create` with Link off on a lone engine (a mesh makes a mesh link instead).
const LINK_OFF: &str = "link is off: turn on Settings › Link to share documents";

/// 184 §7.1, §7.2: a share link's ids, the fragment before `.<doc>.<secret>`, and how the guest
/// gets in. `mesh`: the mesh link id when known. Link off here (only in a mesh, Sky Q3) → the v1
/// mesh link `1.<mesh link id>`. Link on, `home_here` (this engine is the document's home in a
/// mesh with peers, W-j) → `2.<home link id>.<mesh link id>`. Otherwise today's v1: the mesh
/// device id Link serves, else this engine's link id.
fn link_ids(
    status: &LinkStatus,
    mesh: Option<&str>,
    home_here: bool,
) -> RpcResult<(String, document::LinkVia)> {
    use document::LinkVia::{Home, Mesh};
    if !status.enabled {
        return mesh
            .map(|m| (format!("1.{m}"), Mesh))
            .ok_or_else(|| failed(link::MESH_LINK_UNAVAILABLE));
    }
    if home_here && let (Some(m), Some(h)) = (mesh, status.harness_id.as_deref()) {
        return Ok((format!("2.{h}.{m}"), Home));
    }
    match (status.mesh.as_ref(), status.harness_id.as_deref()) {
        (Some(m), _) => Ok((format!("1.{}", m.device_id), Mesh)),
        (None, Some(h)) => Ok((format!("1.{h}"), Home)),
        (None, None) => Err(failed("link has no identity yet")),
    }
}

/// The guest page's origin for relay `relay` (`wss://h` → `https://h`).
fn link_origin(relay: &str) -> String {
    let relay = relay.trim_end_matches('/');
    relay
        .strip_prefix("wss://")
        .map(|h| format!("https://{h}"))
        .or_else(|| relay.strip_prefix("ws://").map(|h| format!("http://{h}")))
        .unwrap_or_else(|| relay.to_owned())
}

/// 184 §7.2 W-v (Z184-R2): only the synced lease holder mints `mesh/device_key`. `is_hub()` alone
/// is also true on both sides of a dual-hub window, and two mints would leave the loser's mesh
/// links dead. The id is read back after the mint, so a row that replicated in meanwhile wins.
async fn mint_as_holder(vault: &secrets::Vault) -> Result<String, String> {
    let holder = mesh::is_hub()
        && mesh::synced()
        && match mesh_actor() {
            Ok(m) => send(&m, mesh::Status)
                .await
                .is_ok_and(|s| s.lease.holder.as_ref() == Some(&s.self_id)),
            Err(_) => false,
        };
    link::mint_mesh_link_id(vault, holder).await?;
    link::mesh_link_id(vault)?.ok_or_else(|| "the mesh has no link id yet".to_owned())
}

/// 184 §7.2 (W-v, Sky Q3): the mesh link id for a mesh link from a Link-off home: Link's pub-only
/// read; with no `mesh/device_key` yet, the lease holder mints it (this engine when it holds the
/// lease, else its `link_mesh_id`); [`link::MESH_LINK_UNAVAILABLE`] when no holder answers.
async fn mesh_link_id(rt: &Runtime) -> RpcResult<String> {
    if let Ok(id) = send(&link()?, link::MeshLinkId).await {
        return Ok(id);
    }
    if mesh::is_hub() {
        let vault = secrets::handle(rt)
            .await
            .ok_or_else(|| failed("secrets vault unavailable"))?;
        // Not yet synced or not the holder after all: Retry (the dialog's mesh_link_unavailable).
        return mint_as_holder(&vault)
            .await
            .map_err(|_| failed(link::MESH_LINK_UNAVAILABLE));
    }
    let unavailable = || failed(link::MESH_LINK_UNAVAILABLE);
    let holder = send(&mesh_actor()?, mesh::Status)
        .await
        .ok()
        .and_then(|s| s.lease.holder)
        .ok_or_else(unavailable)?;
    // A fresh internal query, not a second hop of the user's (possibly forwarded) call: the
    // holder answers it itself and never forwards it.
    match forward::to_peer::<String>(
        &Extensions::new(),
        &replica::PeerId(holder.0),
        "link_mesh_id",
        [(); 0],
    )
    .await
    {
        Ok(Some(id)) => Ok(id),
        _ => Err(unavailable()),
    }
}

/// 184 §7.2: `doc_link_create` in a mesh with peers. Off the document's server the call is
/// forwarded there; on it: Link on → v2 on the home, today's v1 elsewhere; Link off → a v1 mesh
/// link (Sky Q3), whose id this engine reads and never mints.
async fn doc_link_create_in_mesh(
    rt: &Runtime,
    ext: &Extensions,
    doc: document::DocId,
    role: document::Role,
    name: Option<String>,
    expires_ms: Option<i64>,
) -> RpcResult<document::LinkCreated> {
    let docs = documents()?;
    let params = (&doc, role, &name, expires_ms);
    if let Some(v) = to_doc_server(ext, &docs, &doc, "doc_link_create", params).await? {
        return Ok(v);
    }
    let status: LinkStatus = send(&link()?, link::Status).await?;
    let origin = link_origin(&status.relay);
    // Every id is known before the link is written: no link without a URL.
    let (ids, via) = match status.enabled {
        true => link_ids(&status, None, false)?,
        false => link_ids(&status, Some(&mesh_link_id(rt).await?), false)?,
    };
    let create = document::LinkCreate {
        doc: doc.clone(),
        role,
        name: name.clone(),
        expires_ms,
    };
    let r = match doc_send(&docs, create).await {
        Ok((share, secret)) => {
            // v2 only from the home itself (W-j), and only with Link on here.
            let home_here = status.enabled
                && doc_send(
                    &docs,
                    document::MetaOf {
                        doc: doc.clone(),
                        by: document::Principal::Owner { device: None },
                    },
                )
                .await
                .ok()
                .and_then(|m| m.home)
                .is_some_and(|h| mesh::self_id().is_some_and(|me| me == h.0));
            let mesh_id = match (home_here, status.mesh.as_ref()) {
                (false, _) => None,
                (true, Some(m)) => Some(m.device_id.clone()),
                (true, None) => send(&link()?, link::MeshLinkId).await.ok(),
            };
            let (ids, via) = match mesh_id {
                Some(m) => link_ids(&status, Some(&m), true)?,
                None => (ids, via),
            };
            Ok(document::LinkCreated {
                url: format!("{origin}/d#{ids}.{doc}.{secret}"),
                link: share,
                via,
            })
        }
        Err(e) => Err(e),
    };
    doc_forward(ext, "doc_link_create", params, r).await
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

/// 144 §4.3 (E1): the insight job's model call and skill lookup live in `agent`; the document crate
/// cannot name the agent actor, so this crate — which knows both ends — installs the two closures once
/// (the pattern of `codeindex::map::generator::install_side_caller`). Looked up per call.
fn install_insight_hooks(rt: &Runtime) {
    use document::ingest::insights::{Hooks, InsightReply, install};
    let dir = agent::skills::dir(&rt.data_dir);
    install(Hooks {
        side_call: Arc::new(|call| {
            Box::pin(async move {
                let agents =
                    lookup::<Agents>(agent::AGENT_ACTOR).map_err(|e| e.message().to_owned())?;
                let notebooks: Vec<String> =
                    call.notebooks.iter().map(ToString::to_string).collect();
                agent::notebook_context::insight_side_call(
                    &agents,
                    call.session,
                    &notebooks,
                    call.model,
                    call.system,
                    call.user,
                    call.budget,
                )
                .await
                .map(|r| InsightReply {
                    text: r.text,
                    model: r.model,
                })
            })
        }),
        skill: Arc::new(move |name| agent::notebook_context::insight_skill(&dir, name)),
    });
}

/// A `ContextLevel` as its wire word (`off | mention | summary | full`) — what the agent's store keeps.
fn level_str(level: document::ContextLevel) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
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

/// 183 §10.1: a contact RPC: the A2A actor's answer (it runs without `a2a.enabled` too,
/// W-26), or `off` in a process without the a2a plugin.
async fn contacts<M>(
    rt: &Runtime,
    msg: M,
    off: RpcResult<<<A2aActor as Message<M>>::Reply as Reply>::Ok>,
) -> RpcResult<<<A2aActor as Message<M>>::Reply as Reply>::Ok>
where
    A2aActor: Message<M>,
    M: Send + 'static,
    <<A2aActor as Message<M>>::Reply as Reply>::Error: Display,
{
    match a2a(rt)? {
        Some(actor) => send(&actor, msg).await,
        None => off,
    }
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

/// 184 §12.4: `msg` with engine id `id` replaced by its machine name, else by the id's first 8
/// characters, as the UI shows an unknown machine.
fn named(msg: &str, id: &str, name: Option<&str>) -> String {
    let short: String = id.chars().take(8).collect();
    msg.replace(id, name.unwrap_or(&short))
}

/// `id`'s machine name in `st`: this engine's `self_name` or a peer's `name`.
fn machine_name<'a>(st: &'a mesh::MeshStatus, id: &str) -> Option<&'a str> {
    if st.self_id.0 == id {
        return Some(&st.self_name);
    }
    st.peers
        .iter()
        .find(|p| p.id.0 == id)
        .map(|p| p.name.as_str())
}

/// 160: the engine's cost cube service; `NOT_RUNNING` before the agent plugin started.
fn cost_service() -> RpcResult<&'static agent::CostService> {
    agent::CostService::engine().ok_or_else(|| {
        ErrorObjectOwned::owned(NOT_RUNNING, "costs: agent is not running", None::<()>)
    })
}

/// 158 §15.2: a wave module's `anyhow` error as `ASK_FAILED` (the stubs' "not built yet" too).
fn stub_err(e: anyhow::Error) -> ErrorObjectOwned {
    failed(format!("{e:#}"))
}

/// 158 §7.2: the connection's `x-theuth-client` value for companion-only methods; `None` when
/// the call did not come over the socket (the in-process TUI).
fn client_of(ext: &Extensions) -> Option<&'static str> {
    ext.get::<crate::Client>().map(|c| c.0)
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
    let attached = lookup::<Jobs>(JOBS_ACTOR)?
        .ask(PtyAttach {
            session,
            id: job.clone(),
            size,
        })
        .await
        .map_err(|e| match e {
            SendError::HandlerError(msg) => failed(msg),
            other => failed(other.to_string()),
        });
    // 183 §10.3 (W-2): a job of a session owned elsewhere streams there only.
    if attached.is_err()
        && let Some(owner) = forward::owner_peer(session).await
    {
        return Err(forward::runs_elsewhere(owner.0));
    }
    attached
}

#[async_trait]
impl ApiServer for Server {
    async fn info(&self) -> RpcResult<Info> {
        // 82 §3 / 83 §3: the supervisor's live defaults, not the config file read at startup.
        // 176 E3: one ask, so `info` queues behind at most one supervisor message.
        let agent::InfoDefaults {
            default_model,
            default_effort,
            favorite_models,
            compact_style,
            subagent_timeout_secs,
            max_tool_rounds,
            stream_idle_secs,
            max_salvages,
            communication,
            advisor,
        } = ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetInfoDefaults).await?;
        let approvals =
            ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetApprovals { session: None }).await?;
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
            home_offline_ask_secs: self
                .rt
                .section::<tasks::TasksConfig>("tasks")
                .map(|c| c.home_offline_ask_secs)
                .unwrap_or_else(|_| tasks::TasksConfig::default().home_offline_ask_secs),
            max_tool_rounds,
            stream_idle_secs,
            max_salvages,
            pty_idle_secs,
            name,
            communication,
            app_enabled: apps().is_ok(),
            // 125 §6.5: the supervisor's live advisor defaults (82 §3 rule).
            advisor,
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
        self.authorize_opt_fwd(ext, session).await?;
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
                channel: None,
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
        self.authorize_fwd(ext, session).await?;
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
                channel: None,
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
        let Some(forward::Forwarded(peer)) = ext.get::<forward::Forwarded>() else {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "peer_deliver is mesh-internal",
                None::<()>,
            ));
        };
        let forwarder = Some(peer.0.clone());
        // 183 §6.5: a forwarder that does not own `envelope.from` is unauthorized.
        lookup::<Agents>(agent::AGENT_ACTOR)?
            .ask(PeerDeliver {
                session,
                envelope,
                forwarder,
            })
            .await
            .map_err(|e: SendError<PeerDeliver, _>| match e {
                SendError::HandlerError(agent::PeerDeliverError::NotOwner(r)) => {
                    ErrorObjectOwned::owned(UNAUTHORIZED, r.to_string(), None::<()>)
                }
                other => ErrorObjectOwned::owned(ASK_FAILED, other.to_string(), None::<()>),
            })
    }

    async fn contact_deliver(
        &self,
        ext: &Extensions,
        envelope: ContactEnvelope,
    ) -> RpcResult<PeerReceipt> {
        let Some(forward::Forwarded(peer)) = ext.get::<forward::Forwarded>() else {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "contact_deliver is mesh-internal",
                None::<()>,
            ));
        };
        let forwarder = Some(peer.0.clone());
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ContactDeliver {
                envelope,
                forwarder,
            },
        )
        .await
        .map_err(|e| {
            // 183 §13 row 53: the chat moved; the holder retries once on the new owner.
            if e.message()
                .starts_with(agent::communication::network::CONTACT_NOT_OWNER)
            {
                ErrorObjectOwned::owned(forward::NOT_OWNER, e.message().to_owned(), None::<()>)
            } else {
                e
            }
        })
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
        self.authorize_fwd(ext, session).await?;
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
        reason: Option<agent::CancelReason>,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        // 186 (D8): the reason travels with a forwarded cancel; a plain Stop keeps the old
        // two-parameter call, so an engine that predates the reason still takes it.
        let routed = match &reason {
            Some(_) => forward::route(ext, session, "cancel", (session, &scope, &reason)).await?,
            None => forward::route(ext, session, "cancel", (session, &scope)).await?,
        };
        if let Some(r) = routed {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Cancel {
                session,
                keep: None,
                scope: scope.unwrap_or_default(),
                reason: reason.unwrap_or_default(),
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "undo", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Undo { session }).await
    }

    async fn redo(&self, ext: &Extensions, session: SessionId) -> RpcResult<String> {
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "session_memory", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SessionMemoryQuery { session }).await
    }

    async fn endless_retry(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "endless_retry", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::EndlessRetry { session }).await
    }

    async fn endless_fold(&self, ext: &Extensions, session: SessionId) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "endless_fold", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::EndlessFold { session }).await
    }

    async fn endless_pause(
        &self,
        ext: &Extensions,
        session: SessionId,
        paused: bool,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "endless_pause", (session, paused)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::EndlessPause { session, paused }).await
    }

    async fn set_session_compactor_model(
        &self,
        ext: &Extensions,
        session: SessionId,
        model: Option<String>,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "set_session_compactor_model",
            (session, model.clone()),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::SetSessionCompactorModel { session, model },
        )
        .await
    }

    async fn endless_zoom(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: u64,
        n: u64,
    ) -> RpcResult<Vec<ViewLine>> {
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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

    async fn polish_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetPolishModel).await
    }

    async fn set_polish_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetPolishModel { model }).await
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

    async fn theuth_agent_model(&self) -> RpcResult<Option<ModelInfo>> {
        send(&roster_actor()?, roster::TheuthModel).await
    }

    async fn set_theuth_agent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        send(&roster_actor()?, roster::SetTheuthModel { model }).await
    }

    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions).await
    }

    async fn list_agents(&self, ext: &Extensions, session: SessionId) -> RpcResult<Vec<AgentInfo>> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::to_creator(ext, session, "list_agents", (session,)).await? {
            return Ok(r);
        }
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
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::to_creator(
            ext,
            session,
            "load_session",
            (session, before, limit, settled_only),
        )
        .await?
        {
            return Ok(r);
        }
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
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::to_creator(ext, session, "get_todos", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, GetTodos { session }).await
    }

    async fn path_complete(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        input: String,
        cursor: usize,
        engine: Option<String>,
    ) -> RpcResult<PathCompletion> {
        peer(ext)?;
        self.authorize_opt(ext, session).await?;
        // 183 W-12: "Browse xeus…" with no session yet completes on that engine
        // (nothing when it cannot answer, never this machine's paths).
        if session.is_none() {
            match forward::to_engine::<PathCompletion>(
                ext,
                engine.as_deref(),
                "path_complete",
                (None::<SessionId>, &input, cursor),
            )
            .await
            {
                Ok(None) => {}
                Ok(Some(r)) => return Ok(r),
                Err(_) => return Ok(PathCompletion::default()),
            }
        }
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

    async fn cost_cube(&self, since: Option<agent::CostCubeCursor>) -> RpcResult<agent::CostCube> {
        Ok(cost_service()?.cube(since.as_ref()))
    }

    async fn cost_calls(&self, query: agent::CostCallsQuery) -> RpcResult<agent::CostCallsPage> {
        cost_service()?
            .calls(&query)
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn usage_limits(&self) -> RpcResult<agent::limits::UsageLimits> {
        Ok(agent::limits::LimitsHub::global().snapshot())
    }

    async fn usage_limits_refresh(&self, provider: Option<String>) -> RpcResult<()> {
        agent::limits::LimitsHub::global().kick(provider.as_deref());
        Ok(())
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
                format!("reason {reason:?}: expected user | orphan | write | moved"),
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
                // 181: `moved` counts only from the session's owner (the engine forwarding it).
                caller: ext.get::<forward::Forwarded>().map(|f| f.0.clone()),
            },
        )
        .await?;
        Ok(SessionOwner {
            owner,
            epoch,
            here: true,
        })
    }

    async fn session_move(
        &self,
        ext: &Extensions,
        session: SessionId,
        to: String,
    ) -> RpcResult<SessionOwner> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let connected =
            ask::<sessionsync::SessionSync, _>(sessionsync::SESSIONS_ACTOR, sessionsync::Peers)
                .await?;
        // The pause map is this engine's, in memory: the target could not refuse it.
        let held = ask::<Agents, _>(agent::AGENT_ACTOR, agent::supervisor::HeldDesks).await?;
        let peers = match mesh_actor() {
            Ok(m) => m
                .ask(mesh::Status)
                .await
                .map(|s| s.peers)
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let name = peers
            .iter()
            .find(|p| p.id.0 == to)
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| to.get(..8).unwrap_or(&to).to_owned());
        let to = replica::PeerId(to);
        // The take's worst case: release reply (release wait + drain), then the tail wait.
        let sessions = self
            .rt
            .section::<sessionsync::SessionsConfig>("sessions")
            .unwrap_or_default();
        let drain = self
            .rt
            .section::<agent::durability::DurabilityConfig>("durability")
            .unwrap_or_default()
            .drain_secs;
        let wait = std::time::Duration::from_secs(2 * sessions.release_wait_secs + drain + 15);
        // 183 §4.2: Move to on a viewer runs on the owner, which moves it as
        // today (one hop: a forwarded move for a row not ours is NotOwner).
        let me = store.engine().to_owned();
        if let Some((owner, _)) = store
            .owner(session)
            .await
            .map_err(|e| failed(format!("{e:#}")))?
            && !owner.is_empty()
            && owner != me
        {
            if ext.get::<forward::Forwarded>().is_some() {
                return Err(forward::not_owner(owner));
            }
            let owner = replica::PeerId(owner);
            let call = forward::to_peer_within::<SessionOwner>(
                ext,
                &owner,
                "session_move",
                (session, &to.0),
                wait + std::time::Duration::from_secs(10),
            );
            return match call.await? {
                Some(mut o) => {
                    o.here = o.owner == me;
                    Ok(o)
                }
                None => Err(forward::unreachable(owner.0, "not connected").await),
            };
        }
        // The target's own take: it releases from us (drain) and lands epoch + 1.
        // Its own call even when this move was forwarded from a viewer (the
        // take is the target's, bound to us as the owner, not a relay).
        let direct = Extensions::new();
        let ask_target = async {
            let call = forward::to_peer_within::<SessionOwner>(
                &direct,
                &to,
                "session_take",
                (session, "moved"),
                wait + std::time::Duration::from_secs(5),
            );
            match tokio::time::timeout(wait, call).await {
                Err(_) => Err(sessionsync::owner::MoveFailure::NoAnswer),
                Ok(Ok(o)) => Ok(o.map(|o| (o.owner, o.epoch))),
                Ok(Err(e)) if e.code() == forward::OWNER_UNREACHABLE => Ok(None),
                Ok(Err(e)) => Err(sessionsync::owner::MoveFailure::Refused(
                    e.message().to_owned(),
                )),
            }
        };
        let (owner, epoch) =
            sessionsync::owner::move_to(&store, session, &to, &name, &connected, &held, ask_target)
                .await
                .map_err(failed)?;
        tracing::info!(%session, to = %name, epoch, "181: session moved");
        Ok(SessionOwner {
            owner,
            epoch,
            here: false,
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
        match forward::route(ext, session, "archive_session", (session, archived)).await? {
            Some(r) => r,
            None => {
                ask::<Agents, _>(agent::AGENT_ACTOR, ArchiveSession { session, archived }).await?
            }
        }
        // 183 W-41, §12.10: an exported chat's export row reads Paused while it is archived;
        // this engine holds the export (contacts are local), so `A2aChanged` fires here.
        if let Ok(Some(actor)) = a2a(&self.rt) {
            let _ = actor.ask(a2a::contacts::ChatArchived { session }).await;
        }
        Ok(())
    }

    async fn delete_session(&self, ext: &Extensions, session: SessionId) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
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
        // 183 §5.6 (W-10): New chat on another machine: that engine creates and
        // owns the row from epoch 1. Until the row replicates here, calls for it
        // are forwarded there by id (and its reads while it is in use). A
        // private tree belongs to this window's connection and never
        // replicates: it cannot start elsewhere.
        if let Some(engine) = options.engine.clone() {
            if options.retention == Retention::Private && !engine.is_empty() {
                let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
                if engine != store.engine() {
                    return Err(failed(
                        "a private chat runs on this machine; it cannot start on another",
                    ));
                }
            }
            let mut there = options.clone();
            there.engine = None;
            if let Some(open) =
                forward::to_engine::<SessionOpen>(ext, Some(&engine), "create_session", (there,))
                    .await?
            {
                sessionsync::owner::expect(open.session, replica::PeerId(engine.clone()));
                self.wait_for_row(open.session, &engine).await;
                return Ok(open);
            }
        }
        // A forwarded create names a folder of this machine (§13 row 12).
        if ext.get::<forward::Forwarded>().is_some()
            && let Some(cwd) = options.cwd.as_deref()
            && !Path::new(cwd).is_dir()
        {
            return Err(failed(format!(
                "{cwd} does not exist on {}",
                platform::process::machine().name
            )));
        }
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
        self.authorize_fwd(ext, session).await?;
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

    async fn answer_fence_snapshot(
        &self,
        id: artifact::snapshot::FenceSnapshotId,
        answer: artifact::snapshot::FenceSnapshotAnswer,
    ) -> RpcResult<bool> {
        Ok(artifact::snapshot::answer(id, answer))
    }

    async fn list_models(
        &self,
        ext: &Extensions,
        engine: Option<String>,
    ) -> RpcResult<Vec<ModelInfo>> {
        // 183 W-12: another engine's catalog (New chat on a machine, the ⋯ model list).
        if let Some(r) =
            forward::to_engine(ext, engine.as_deref(), "list_models", Vec::<()>::new()).await?
        {
            return Ok(r);
        }
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
        self.authorize_opt_fwd(ext, session).await?;
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
        self.authorize_opt_fwd(ext, session).await?;
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

    async fn set_advisor_max_consults(&self, n: u8) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                max_consults: Some(n),
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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

    async fn resume_child(&self, ext: &Extensions, session: SessionId) -> RpcResult<ChildResumed> {
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, session).await?;
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
        clear: Option<bool>,
    ) -> RpcResult<ApprovalsState> {
        self.authorize_opt_fwd(ext, session).await?;
        // 183 §4.5: run on the owner, which persists the pin on the row. `clear`
        // travels only when set, so a pre-183 owner still answers the plain call.
        if let Some(s) = session {
            let forwarded = match clear {
                Some(c) => forward::route(ext, s, "set_approvals", (session, enabled, c)).await?,
                None => forward::route(ext, s, "set_approvals", (session, enabled)).await?,
            };
            if let Some(r) = forwarded {
                return Ok(r);
            }
        }
        ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            SetApprovals {
                session,
                enabled,
                clear: clear.unwrap_or(false),
            },
        )
        .await
    }

    async fn get_approvals(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<ApprovalsState> {
        self.authorize_opt_fwd(ext, session).await?;
        // 183 W-5: the effective value mixes the row's pin with the owner's
        // `tools.approvals`; owner offline → `-32010` (183-notes deviation 7).
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "get_approvals", (session,)).await?
        {
            return Ok(r);
        }
        // The row is the pin's truth (D4); the registry holds it only once the
        // session's actor spawned here, so a session idle since a restart or a
        // move would read "inherit". ponytail: a setter racing this read can be
        // overwritten by the row it replaced; both are user clicks.
        if let Some(s) = session
            && let Ok(store) = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await
            && let Ok(Some((pin, _))) = store.session_settings(s).await
        {
            let _ = ask::<ToolRegistry, _>(
                tools::REGISTRY_ACTOR,
                tools::RestoreApprovals { session: s, pin },
            )
            .await;
        }
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetApprovals { session }).await
    }

    async fn get_communication(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<CommunicationState> {
        self.authorize_opt_fwd(ext, session).await?;
        // 183 W-5: the owner's `agent.communication` decides.
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "get_communication", (session,)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, GetCommunication { session }).await
    }

    async fn set_communication(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<CommunicationState> {
        self.authorize_opt_fwd(ext, session).await?;
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
        self.authorize_fwd(ext, root).await?;
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
                kind: None,
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
        self.authorize_fwd(ext, session).await?;
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
        self.authorize_opt_fwd(ext, session).await?;
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
        // 171 §4.4: the user's export carries each cell's last output and run line.
        let markdown = document::reader_markdown(&docs, doc, None, annotate)
            .await
            .map_err(|e| failed(e.to_string()))?;
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
        // 184 §7.2: in a mesh the document's server makes the link; a lone engine (paired phones
        // or not) runs today's code unchanged.
        if mesh::has_peers() == Some(true) {
            return doc_link_create_in_mesh(&self.rt, ext, doc, role, name, expires_ms).await;
        }
        // The URL needs the relay host and an id devices dial (59 §8.2): Link must be on. 69: the mesh
        // device id when this engine has one — the hub, whichever engine it is, answers `/h/<mesh id>`
        // — else the engine id (a single engine before any device paired). Old engine-id links keep
        // resolving: the engine uplink stays registered.
        let status: LinkStatus = send(&link()?, link::Status).await?;
        if !status.enabled {
            return Err(failed(LINK_OFF));
        }
        let (ids, via) = link_ids(&status, None, false)?;
        let origin = link_origin(&status.relay);
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
            url: format!("{origin}/d#{ids}.{doc}.{secret}"),
            link,
            via,
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
        join_outcome(guest, &r);
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
        let (peer, by) = principal(ext)?;
        let mut r = doc_send(&documents()?, document::NotebookGet { id, by }).await;
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
        let (peer, by) = principal(ext)?;
        let r = doc_send(&documents()?, document::NotebookItemsList { notebook, by }).await;
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
        let (peer, by) = principal(ext)?;
        // 144 §11: readable by the owner, a member of the source, or a member of a notebook holding it.
        let docs = documents()?;
        let may = doc_send(
            &docs,
            document::MayRead {
                doc: id.clone(),
                by,
            },
        )
        .await;
        member_outcome(&peer, &may);
        may?;
        let r = nb_result(document::ingest::source_get(&docs, id).await);
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

    async fn notebook_context_set(
        &self,
        ext: &Extensions,
        session: SessionId,
        item: String,
        kind: document::ItemKind,
        level: document::ContextLevel,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::notebook_context::context_set(
                &agents,
                session,
                item,
                kind.as_str().to_owned(),
                level_str(level),
            )
            .await,
        )
    }

    async fn notebook_ask(
        &self,
        ext: &Extensions,
        scope: Vec<document::DocId>,
        question: String,
        save_into: Option<document::DocId>,
    ) -> RpcResult<agent::ask::AskStarted> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::ask::notebook_ask(
                &agents,
                scope.iter().map(ToString::to_string).collect(),
                question,
                save_into.map(|n| n.to_string()),
            )
            .await,
        )
    }

    async fn notebook_context_clear(
        &self,
        ext: &Extensions,
        session: SessionId,
        item: String,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::notebook_context::context_clear(&agents, session, item).await)
    }

    async fn notebook_context_set_all(
        &self,
        ext: &Extensions,
        session: SessionId,
        notebook: document::DocId,
        kind: Option<document::ItemKind>,
        level: document::ContextLevel,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::notebook_context::context_set_all(
                &agents,
                session,
                notebook.to_string(),
                kind.map(|k| k.as_str().to_owned()),
                level_str(level),
            )
            .await,
        )
    }

    async fn notebook_context_estimate(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
        session: Option<SessionId>,
    ) -> RpcResult<document::ContextEstimate> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        let est = nb_result(
            agent::notebook_context::estimate(&agents, session, &notebook.to_string(), &self.rt)
                .await,
        )?;
        // The agent's estimate serialises to the contract's shape (string kinds, lowercase levels).
        serde_json::to_value(&est)
            .and_then(serde_json::from_value)
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>))
    }

    async fn insight_run(
        &self,
        ext: &Extensions,
        source: document::DocId,
        instruction: document::InsightInstruction,
    ) -> RpcResult<document::InsightRun> {
        owner(ext)?;
        let r = nb_result(
            document::ingest::insights::insight_run(
                &documents()?,
                source.clone(),
                instruction.clone(),
                None,
            )
            .await,
        );
        doc_forward(ext, "insight_run", (&source, &instruction), r).await
    }

    async fn insight_promote(
        &self,
        ext: &Extensions,
        source: document::DocId,
        block: String,
        notebook: Option<document::DocId>,
    ) -> RpcResult<document::DocId> {
        owner(ext)?;
        let r = nb_result(
            document::ingest::insights::insight_promote(
                &documents()?,
                source.clone(),
                block.clone(),
                notebook.clone(),
                document::Principal::Owner { device: None },
            )
            .await,
        );
        doc_forward(ext, "insight_promote", (&source, &block, &notebook), r).await
    }

    async fn insight_skills(&self) -> RpcResult<Vec<String>> {
        let mut out = document::ingest::insights::builtin_names();
        out.extend(agent::notebook_context::insight_skills(
            &agent::skills::dir(&self.rt.data_dir),
        ));
        Ok(out)
    }

    async fn notebook_ask_list(
        &self,
        ext: &Extensions,
        limit: Option<u32>,
    ) -> RpcResult<Vec<agent::ask::AskSummary>> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::ask::notebook_ask_list(&agents, limit).await)
    }

    async fn notebook_overview_run(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
        kind: agent::overviews::OverviewKind,
    ) -> RpcResult<agent::overviews::OverviewRun> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::overviews::notebook_overview_run(&agents, notebook.to_string(), kind).await,
        )
    }

    async fn notebook_overviews(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
    ) -> RpcResult<Vec<agent::overviews::Overview>> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::overviews::notebook_overviews(&agents, notebook.to_string()).await)
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
                home: document::Homing::Here,
            },
        )
        .await;
        doc_forward(ext, "doc_create", (&title, &markdown), r).await
    }

    async fn doc_move_home(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        to: String,
    ) -> RpcResult<document::DocSummary> {
        owner(ext)?;
        // §4.8: the note's server runs the checks and hands the live copy over.
        let docs = documents()?;
        if let Some(v) = to_doc_server(ext, &docs, &doc, "doc_move_home", (&doc, &to)).await? {
            return Ok(v);
        }
        let r = doc_send(
            &docs,
            document::MoveHome {
                doc,
                to: mesh::EngineId(to.clone()),
                checks: true,
            },
        )
        .await;
        // §12.4: the actor's copy names engine ids; the user reads machine names.
        match r {
            Err(e) if e.message().contains(&to) => {
                let st = match mesh_actor() {
                    Ok(m) => send(&m, mesh::Status).await.ok(),
                    Err(_) => None,
                };
                let name = st.as_ref().and_then(|st| machine_name(st, &to));
                Err(failed(named(e.message(), &to, name)))
            }
            r => r,
        }
    }

    async fn link_mesh_id(&self, ext: &Extensions) -> RpcResult<String> {
        if ext.get::<forward::Forwarded>().is_none() {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "link_mesh_id is mesh-internal",
                None::<()>,
            ));
        }
        // W-v: only the lease holder mints `mesh/device_key`; `not the hub` elsewhere.
        let vault = secrets::handle(&self.rt)
            .await
            .ok_or_else(|| failed("secrets vault unavailable"))?;
        mint_as_holder(&vault).await.map_err(failed)
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
        self.authorize_fwd(ext, session).await?;
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

    async fn codemap_overview(&self, root: String) -> RpcResult<MapIndex> {
        send(
            &codeindex()?,
            codeindex::map::Overview {
                root: PathBuf::from(root),
            },
        )
        .await
    }

    async fn codemap_element(&self, root: String, element: String) -> RpcResult<MapElement> {
        send(
            &codeindex()?,
            codeindex::map::Element {
                root: PathBuf::from(root),
                element,
            },
        )
        .await
    }

    async fn codemap_generate(
        &self,
        root: String,
        session: SessionId,
        element: Option<String>,
    ) -> RpcResult<MapJob> {
        send(
            &codeindex()?,
            codeindex::map::Generate {
                root: PathBuf::from(root),
                session,
                element,
            },
        )
        .await
    }

    async fn codemap_cancel(&self, root: String) -> RpcResult<MapJob> {
        send(
            &codeindex()?,
            codeindex::map::Cancel {
                root: PathBuf::from(root),
            },
        )
        .await
    }

    async fn codemap_search(
        &self,
        root: String,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<MapHit>> {
        send(
            &codeindex()?,
            codeindex::map::Search {
                root: PathBuf::from(root),
                query,
                limit,
            },
        )
        .await
    }

    async fn codemap_export(&self, root: String) -> RpcResult<MapExport> {
        send(
            &codeindex()?,
            codeindex::map::Export {
                root: PathBuf::from(root),
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
        key: Option<String>,
        runtime: Option<agent::LocalRuntime>,
    ) -> RpcResult<ProviderInfo> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            AddProvider {
                name,
                base_url,
                key,
                runtime,
            },
        )
        .await
    }

    async fn remove_provider(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveProvider { name }).await
    }

    async fn refresh_provider_models(&self, name: String) -> RpcResult<RefreshedModels> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RefreshProviderModels { name }).await
    }

    async fn local_servers_probe(&self) -> RpcResult<Vec<agent::LocalServer>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::ProbeLocalServers).await
    }

    async fn provider_models(&self, provider: String) -> RpcResult<Vec<agent::ProviderModel>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::ProviderModels { provider }).await
    }

    async fn set_model_limits(
        &self,
        provider: String,
        model: String,
        window: Option<u32>,
        output: Option<u32>,
    ) -> RpcResult<agent::ProviderModel> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::SetModelLimits {
                provider,
                model,
                context: window,
                output,
            },
        )
        .await
    }

    async fn test_summary_call(&self, model: String) -> RpcResult<agent::SummaryTest> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::TestSummaryCall { model }).await
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

    async fn slack_manifest(&self) -> RpcResult<String> {
        Ok(slack::MANIFEST.to_owned())
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
        let mut status = send(&voice()?, voice::Status).await?;
        // The live list (`set_input_devices`), not the one read at startup.
        if let Ok(d) = dictation() {
            status.input_devices = send(&d, voice::InputDevicePrefs).await?;
        }
        Ok(status)
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
        opts: Option<voice::DictationOpts>,
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
        let id = send(
            &actor,
            voice::StartDictation {
                source,
                opts: opts.unwrap_or_default(),
            },
        )
        .await?;
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

    // ---- 153 dictation history / dictionary (A4 fills the bodies) ----------------------------

    async fn dictation_correction(
        &self,
        ext: &Extensions,
        id: voice::DictationId,
        sent_text: String,
    ) -> RpcResult<()> {
        peer(ext)?;
        send(&dictation()?, voice::DictationCorrection { id, sent_text }).await
    }

    async fn dictation_history(
        &self,
        ext: &Extensions,
        limit: Option<u32>,
    ) -> RpcResult<Vec<voice::HistoryRow>> {
        let store = history_store(ext).await?;
        store
            .list(limit.unwrap_or(50))
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn dictation_history_forget(
        &self,
        ext: &Extensions,
        id: Option<voice::DictationId>,
    ) -> RpcResult<()> {
        let store = history_store(ext).await?;
        store.forget(id).await.map_err(|e| failed(format!("{e:#}")))
    }

    async fn dictation_audio_get(
        &self,
        ext: &Extensions,
        id: voice::DictationId,
        utterance: Option<u32>,
    ) -> RpcResult<String> {
        let store = history_store(ext).await?;
        let pcm = store
            .audio(id, utterance)
            .await
            .map_err(|e| failed(format!("{e:#}")))?;
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    async fn dictionary_list(&self, scope: Option<voice::Scope>) -> RpcResult<Vec<voice::Entry>> {
        match send(&dictation()?, voice::DictionaryOp::List(scope)).await? {
            voice::DictionaryReply::Entries(e) => Ok(e),
            voice::DictionaryReply::Loading => Err(failed("dictionary loading")),
            other => Err(failed(format!("unexpected dictionary reply {other:?}"))),
        }
    }

    async fn dictionary_put(&self, entry: voice::Entry) -> RpcResult<voice::Entry> {
        match send(&dictation()?, voice::DictionaryOp::Put(entry)).await? {
            voice::DictionaryReply::Entry(e) => Ok(e),
            voice::DictionaryReply::Loading => Err(failed("dictionary loading")),
            other => Err(failed(format!("unexpected dictionary reply {other:?}"))),
        }
    }

    async fn dictionary_delete(&self, id: voice::EntryId) -> RpcResult<bool> {
        match send(&dictation()?, voice::DictionaryOp::Delete(id)).await? {
            voice::DictionaryReply::Deleted(d) => Ok(d),
            voice::DictionaryReply::Loading => Err(failed("dictionary loading")),
            other => Err(failed(format!("unexpected dictionary reply {other:?}"))),
        }
    }

    async fn dictionary_suggestion_answer(
        &self,
        id: voice::SuggestionId,
        answer: voice::SuggestionAnswer,
    ) -> RpcResult<()> {
        send(&dictation()?, voice::AnswerSuggestion { id, answer }).await
    }

    async fn list_input_devices(&self) -> RpcResult<Vec<voice::InputDevice>> {
        // Host enumeration is a blocking call into the audio backend.
        tokio::task::spawn_blocking(voice::mic::list_input_devices)
            .await
            .map_err(|e| failed(format!("device list: {e}")))
    }

    async fn set_input_devices(&self, names: Vec<String>) -> RpcResult<()> {
        let value = serde_json::to_string(&names).map_err(|e| failed(e.to_string()))?;
        self.write_voice_config(&["input_devices"], &value)?;
        send(&dictation()?, voice::Setting::InputDevices(names)).await
    }

    async fn set_shape(&self, surface: voice::Surface, preset: voice::Preset) -> RpcResult<()> {
        let key = serde_json::to_value(surface)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| failed("bad surface"))?;
        let value = serde_json::to_value(preset)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| failed("bad preset"))?;
        self.write_voice_config(&["shape", &key], &value)?;
        send(&dictation()?, voice::Setting::Shape(surface, preset)).await
    }

    async fn set_end_phrases(&self, phrases: voice::EndPhrases) -> RpcResult<()> {
        let json = |v: serde_json::Value| v.to_string();
        self.write_voice_config(
            &["end_commands", "send"],
            &json(phrases.send.clone().into()),
        )?;
        self.write_voice_config(
            &["end_commands", "scratch"],
            &json(phrases.scratch.clone().into()),
        )?;
        self.write_voice_config(
            &["end_commands", "literal"],
            &json(phrases.literal.clone().into()),
        )?;
        send(&dictation()?, voice::Setting::EndPhrases(phrases)).await
    }

    async fn voice_settings(&self) -> RpcResult<voice::VoiceSettings> {
        let mut settings = send(&dictation()?, voice::CurrentSettings).await?;
        // The live slot (`set_polish_model`), not the one read at startup.
        settings.polish_model = ask::<Agents, _>(agent::AGENT_ACTOR, GetPolishModel)
            .await?
            .map(|m| m.id);
        Ok(settings)
    }

    // ---- 153 polish / rewrite (A5 fills the bodies) ------------------------------------------

    async fn polish(
        &self,
        text: String,
        destination: voice::Destination,
        session: Option<SessionId>,
    ) -> RpcResult<PolishReply> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Polish {
                text,
                destination,
                session,
            },
        )
        .await
    }

    async fn rewrite(
        &self,
        instruction: String,
        selection: String,
        app: voice::AppProfile,
        thread: Option<Vec<(String, String)>>,
    ) -> RpcResult<PolishReply> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Rewrite {
                instruction,
                selection,
                app,
                thread,
            },
        )
        .await
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

    async fn meeting_list(
        &self,
        limit: Option<u32>,
        project: Option<String>,
    ) -> RpcResult<Vec<meeting::MeetingSummary>> {
        send(&meeting()?, meeting::ListMeetings { limit, project }).await
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

    async fn meeting_speaker_enroll(
        &self,
        id: meeting::MeetingId,
        segments: Vec<u32>,
        target: meeting::EnrollTarget,
    ) -> RpcResult<meeting::Enrolled> {
        send(
            &meeting()?,
            meeting::EnrollSegments {
                meeting: id,
                segments,
                target,
            },
        )
        .await
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

    async fn dreaming_run_now(
        &self,
        ext: &Extensions,
        request: dreaming::RunRequest,
    ) -> RpcResult<SessionId> {
        // 185 A5: the hub dreams; forwarded once (`to_peer` never makes a second
        // hop), and run here when the hub is not connected (this side is cut off).
        if let Some(hub) = dreaming::rpc::run_now_host().await
            && let Some(s) =
                forward::to_peer(ext, &replica::PeerId(hub), "dreaming.run_now", [&request])
                    .await
                    .map_err(hub_said)?
        {
            return Ok(s);
        }
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

    // 140 §12.5: every method asks the `autoresearch` actor through `autoresearch::rpc_impl`
    // (`dreaming_err` maps "is not running" to `NOT_RUNNING`); `list` is empty and
    // `settings.get` / `settings.set` answer from the section when the plugin is disabled, so
    // the Library tab and Settings › Autoresearch work (and can re-enable it) without the actor.

    async fn autoresearch_list(&self) -> RpcResult<Vec<autoresearch::StudyInfo>> {
        match autoresearch::rpc_impl::list().await {
            Err(e) if e.ends_with("is not running") => Ok(Vec::new()),
            r => r.map_err(dreaming_err),
        }
    }

    async fn autoresearch_get(&self, slug: String) -> RpcResult<autoresearch::StudyDetail> {
        autoresearch::rpc_impl::get(slug)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_experiments(
        &self,
        slug: String,
        before: Option<u32>,
        limit: u32,
    ) -> RpcResult<Vec<autoresearch::ExperimentRow>> {
        autoresearch::rpc_impl::experiments(slug, before, limit)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_experiment(
        &self,
        slug: String,
        n: u32,
    ) -> RpcResult<autoresearch::ExperimentDetail> {
        autoresearch::rpc_impl::experiment(slug, n)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_control(
        &self,
        slug: String,
        op: autoresearch::ControlOp,
    ) -> RpcResult<autoresearch::StudyInfo> {
        // 159 §13.3: a learned-sidecar study's Merge lands through dreaming first (backup,
        // atomic write, `edits.jsonl`); the actor's Merge then does its bookkeeping.
        crate::landing::before_merge(&self.rt.data_dir, &slug, &op)
            .await
            .map_err(dreaming_err)?;
        autoresearch::rpc_impl::control(slug, op)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_steer(&self, slug: String, text: String) -> RpcResult<i64> {
        autoresearch::rpc_impl::steer(slug, text)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_idea(
        &self,
        slug: String,
        op: autoresearch::IdeaChange,
    ) -> RpcResult<autoresearch::Idea> {
        autoresearch::rpc_impl::idea(slug, op)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_create(
        &self,
        draft_id: String,
        start: autoresearch::StartWhen,
    ) -> RpcResult<autoresearch::StudyInfo> {
        autoresearch::rpc_impl::create(draft_id, start)
            .await
            .map_err(dreaming_err)
    }

    // 151 §4.4: the living draft (bodies in `autoresearch::rpc_impl`, built by ENG).

    async fn autoresearch_draft(&self, draft_id: String) -> RpcResult<autoresearch::DraftView> {
        autoresearch::rpc_impl::draft(draft_id)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_drafts(&self) -> RpcResult<Vec<autoresearch::DraftSummary>> {
        match autoresearch::rpc_impl::drafts().await {
            Err(e) if e.ends_with("is not running") => Ok(Vec::new()),
            r => r.map_err(dreaming_err),
        }
    }

    async fn autoresearch_draft_edit(
        &self,
        draft_id: String,
        field: String,
        value: String,
    ) -> RpcResult<autoresearch::DraftView> {
        autoresearch::rpc_impl::draft_edit(draft_id, field, value)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_draft_confirm(
        &self,
        draft_id: String,
        field: String,
    ) -> RpcResult<autoresearch::DraftView> {
        autoresearch::rpc_impl::draft_confirm(draft_id, field)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_discard_draft(&self, draft_id: String) -> RpcResult<()> {
        autoresearch::rpc_impl::discard_draft(draft_id)
            .await
            .map_err(dreaming_err)
    }

    // 159 §5.11: per-kind methods; stubs in `autoresearch::rpc_impl` until their wave.

    async fn autoresearch_archive(&self, slug: String) -> RpcResult<Vec<autoresearch::ArchiveRow>> {
        autoresearch::rpc_impl::archive(slug)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_cases(
        &self,
        slug: String,
        n: Option<u32>,
    ) -> RpcResult<Vec<autoresearch::CaseRow>> {
        autoresearch::rpc_impl::cases(slug, n)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_judgments(
        &self,
        slug: String,
        n: Option<u32>,
    ) -> RpcResult<Vec<autoresearch::JudgmentRow>> {
        autoresearch::rpc_impl::judgments(slug, n)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_human_pair(
        &self,
        slug: String,
        id: i64,
        winner: String,
    ) -> RpcResult<autoresearch::StudyInfo> {
        autoresearch::rpc_impl::human_pair(slug, id, winner)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_launch(
        &self,
        slug: String,
        n: u32,
        token: String,
        approve: bool,
    ) -> RpcResult<autoresearch::StudyInfo> {
        autoresearch::rpc_impl::launch(slug, n, token, approve)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_mechanism(
        &self,
        op: autoresearch::MechanismOp,
    ) -> RpcResult<autoresearch::MechanismInfo> {
        autoresearch::rpc_impl::mechanism(op)
            .await
            .map_err(dreaming_err)
    }

    // 171:C cells (design 171 §15.2): one delegating line per method.
    async fn cell_run(
        &self,
        ext: &Extensions,
        cell: cells::CellRef,
        seen: String,
        mode: cells::RunMode,
        run: cells::RunId,
        edited: Option<String>,
        member_ack: Option<String>,
        confirmed: Option<bool>,
    ) -> RpcResult<cells::CellRun> {
        crate::cells::run(
            self, ext, cell, seen, mode, run, edited, member_ack, confirmed,
        )
        .await
    }

    async fn cell_run_all(
        &self,
        ext: &Extensions,
        scope: cells::Scope,
        cells: Vec<(cells::CellRef, String)>,
        pass: String,
    ) -> RpcResult<String> {
        crate::cells::run_all(self, ext, scope, cells, pass).await
    }

    async fn cell_stop(&self, ext: &Extensions, run: cells::RunId, kill: bool) -> RpcResult<()> {
        crate::cells::stop(self, ext, run, kill).await
    }

    async fn cell_input(&self, ext: &Extensions, run: cells::RunId, data: String) -> RpcResult<()> {
        crate::cells::input(self, ext, run, data).await
    }

    async fn cell_ask(&self, ext: &Extensions, run: cells::RunId) -> RpcResult<()> {
        crate::cells::ask(self, ext, run).await
    }

    async fn cell_attach(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        run: cells::RunId,
    ) -> SubscriptionResult {
        crate::cells::attach(self, pending, ext, run).await
    }

    async fn cell_state(
        &self,
        ext: &Extensions,
        scope: cells::Scope,
        cells: Vec<cells::CellRef>,
    ) -> RpcResult<cells::StateReply> {
        crate::cells::state(self, ext, scope, cells).await
    }

    async fn cell_run_get(&self, ext: &Extensions, run: cells::RunId) -> RpcResult<cells::RunGet> {
        crate::cells::run_get(self, ext, run).await
    }

    async fn cell_output(
        &self,
        ext: &Extensions,
        run: cells::RunId,
        offset: u64,
        limit: u32,
        ansi: bool,
    ) -> RpcResult<cells::OutputPage> {
        crate::cells::output(self, ext, run, offset, limit, ansi).await
    }

    async fn cell_output_file(
        &self,
        ext: &Extensions,
        run: cells::RunId,
        name: String,
    ) -> RpcResult<String> {
        crate::cells::output_file(self, ext, run, name).await
    }

    async fn cell_runs(
        &self,
        ext: &Extensions,
        scope: cells::Scope,
        limit: u32,
    ) -> RpcResult<Vec<cells::CellRun>> {
        crate::cells::runs(self, ext, scope, limit).await
    }

    async fn cell_interpreters(
        &self,
        ext: &Extensions,
        cwd: String,
        engine: Option<String>,
    ) -> RpcResult<Vec<cells::Interpreter>> {
        crate::cells::interpreters(self, ext, cwd, engine).await
    }

    async fn cell_kernel_restart(
        &self,
        ext: &Extensions,
        kernel: cells::KernelId,
    ) -> RpcResult<()> {
        crate::cells::kernel_restart(self, ext, kernel).await
    }

    async fn cell_kernel_close(&self, ext: &Extensions, kernel: cells::KernelId) -> RpcResult<()> {
        crate::cells::kernel_close(self, ext, kernel).await
    }

    async fn cell_env_refresh(&self, ext: &Extensions) -> RpcResult<()> {
        crate::cells::env_refresh(self, ext).await
    }

    async fn cell_live(&self, ext: &Extensions) -> RpcResult<Vec<cells::CellRun>> {
        crate::cells::live(self, ext).await
    }

    async fn cell_latest(
        &self,
        ext: &Extensions,
        doc: document::DocId,
    ) -> RpcResult<Vec<cells::CellLatest>> {
        crate::cells_latest::latest(self, ext, doc).await
    }

    async fn cell_reply_note_get(
        &self,
        ext: &Extensions,
        session: cells::SessionId,
        entry: String,
    ) -> RpcResult<Option<cells::DocId>> {
        crate::cells::reply_note_get(self, ext, session, entry).await
    }

    async fn cell_reply_note_set(
        &self,
        ext: &Extensions,
        session: cells::SessionId,
        entry: String,
        doc: cells::DocId,
    ) -> RpcResult<()> {
        crate::cells::reply_note_set(self, ext, session, entry, doc).await
    }

    async fn autoresearch_settings_get(&self) -> RpcResult<autoresearch::AutoresearchConfig> {
        match autoresearch::rpc_impl::settings().await {
            Ok(c) => Ok(c),
            Err(_) => Ok(self.autoresearch_config()),
        }
    }

    async fn autoresearch_settings_set(
        &self,
        config: autoresearch::AutoresearchConfig,
    ) -> RpcResult<autoresearch::AutoresearchConfig> {
        match autoresearch::rpc_impl::set_settings(config.clone()).await {
            Err(e) if e.ends_with("is not running") => {
                autoresearch::config::persist(&self.rt.data_dir.join("config.yaml"), &config)
                    .map_err(|e| dreaming_err(format!("config.yaml: {e}")))?;
                Ok(config)
            }
            r => r.map_err(dreaming_err),
        }
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

    // 158 §15.2 — contract methods (C158): one delegate call each, `anyhow` → `ASK_FAILED`.

    async fn meeting_detection(&self) -> RpcResult<Option<meeting::Detection>> {
        meeting::detect::rpc_detection(&self.rt)
            .await
            .map_err(stub_err)
    }

    async fn meeting_detection_dismiss(&self, app_id: String, never: bool) -> RpcResult<()> {
        meeting::detect::rpc_detection_dismiss(&self.rt, app_id, never)
            .await
            .map_err(stub_err)
    }

    async fn meeting_detection_set_event(
        &self,
        event: Option<meeting::EventId>,
    ) -> RpcResult<Option<meeting::Detection>> {
        meeting::calendar::rpc_detection_set_event(&self.rt, event)
            .await
            .map_err(stub_err)
    }

    async fn meeting_settings_get(&self) -> RpcResult<meeting::MeetingSettings> {
        meeting::rpc_settings_get(&self.rt).await.map_err(stub_err)
    }

    async fn meeting_settings_set(
        &self,
        settings: meeting::MeetingSettings,
    ) -> RpcResult<meeting::MeetingSettings> {
        meeting::rpc_settings_set(&self.rt, settings)
            .await
            .map_err(stub_err)
    }

    async fn meeting_set_project(
        &self,
        id: meeting::MeetingId,
        project: Option<String>,
    ) -> RpcResult<meeting::Manifest> {
        meeting::calendar::prebrief::rpc_set_project(&self.rt, id, project)
            .await
            .map_err(stub_err)
    }

    async fn meeting_set_event(
        &self,
        id: meeting::MeetingId,
        event: Option<meeting::EventId>,
    ) -> RpcResult<meeting::Manifest> {
        meeting::calendar::rpc_set_event(&self.rt, id, event)
            .await
            .map_err(stub_err)
    }

    async fn meeting_set_template(
        &self,
        id: meeting::MeetingId,
        template: String,
    ) -> RpcResult<meeting::Manifest> {
        meeting::templates::rpc_set_template(&self.rt, id, template)
            .await
            .map_err(stub_err)
    }

    async fn meeting_set_speakers(
        &self,
        id: meeting::MeetingId,
        speakers: bool,
    ) -> RpcResult<meeting::Manifest> {
        meeting::post::echo::rpc_set_speakers(&self.rt, id, speakers)
            .await
            .map_err(stub_err)
    }

    async fn meeting_suggest_speakers(
        &self,
        id: meeting::MeetingId,
    ) -> RpcResult<Vec<meeting::Suggestion>> {
        meeting::post::suggest::rpc_suggest(&self.rt, id)
            .await
            .map_err(stub_err)
    }

    async fn meeting_templates(&self) -> RpcResult<Vec<meeting::Template>> {
        meeting::templates::rpc_templates(&self.rt)
            .await
            .map_err(stub_err)
    }

    async fn meeting_template_put(
        &self,
        slug: String,
        body: String,
    ) -> RpcResult<meeting::Template> {
        meeting::templates::rpc_template_put(&self.rt, slug, body)
            .await
            .map_err(stub_err)
    }

    async fn meeting_template_delete(&self, slug: String) -> RpcResult<()> {
        meeting::templates::rpc_template_delete(&self.rt, slug)
            .await
            .map_err(stub_err)
    }

    async fn meeting_template_reset(&self, slug: String) -> RpcResult<meeting::Template> {
        meeting::templates::rpc_template_reset(&self.rt, slug)
            .await
            .map_err(stub_err)
    }

    async fn calendar_sources(&self) -> RpcResult<Vec<meeting::CalSource>> {
        meeting::calendar::rpc_sources(&self.rt)
            .await
            .map_err(stub_err)
    }

    async fn calendar_source_add(
        &self,
        name: String,
        url: String,
    ) -> RpcResult<meeting::CalSource> {
        meeting::calendar::rpc_source_add(&self.rt, name, url)
            .await
            .map_err(stub_err)
    }

    async fn calendar_source_delete(&self, id: String) -> RpcResult<()> {
        meeting::calendar::rpc_source_delete(&self.rt, id)
            .await
            .map_err(stub_err)
    }

    async fn calendar_source_refresh(&self, id: String) -> RpcResult<meeting::CalSource> {
        meeting::calendar::rpc_source_refresh(&self.rt, id)
            .await
            .map_err(stub_err)
    }

    async fn calendar_push(
        &self,
        ext: &Extensions,
        source: String,
        window: (String, String),
        events: Vec<meeting::CalEvent>,
    ) -> RpcResult<u32> {
        meeting::calendar::rpc_push(&self.rt, client_of(ext), source, window, events)
            .await
            .map_err(stub_err)
    }

    async fn calendar_source_state(
        &self,
        ext: &Extensions,
        source: String,
        state: String,
    ) -> RpcResult<()> {
        meeting::calendar::rpc_source_state(&self.rt, client_of(ext), source, state)
            .await
            .map_err(stub_err)
    }

    async fn calendar_events(&self, from: String, to: String) -> RpcResult<Vec<meeting::CalEvent>> {
        meeting::calendar::rpc_events(&self.rt, from, to)
            .await
            .map_err(stub_err)
    }

    async fn calendar_settings_get(&self) -> RpcResult<meeting::CalendarConfig> {
        meeting::calendar::rpc_settings_get(&self.rt)
            .await
            .map_err(stub_err)
    }

    async fn calendar_settings_set(
        &self,
        config: meeting::CalendarConfig,
    ) -> RpcResult<meeting::CalendarConfig> {
        meeting::calendar::rpc_settings_set(&self.rt, config)
            .await
            .map_err(stub_err)
    }

    async fn mcp_server_get(&self) -> RpcResult<mcp_server::McpServerStatus> {
        mcp_server::status(&self.rt).await.map_err(stub_err)
    }

    async fn mcp_server_set(
        &self,
        config: mcp_server::McpServerConfig,
    ) -> RpcResult<mcp_server::McpServerStatus> {
        mcp_server::set(&self.rt, config).await.map_err(stub_err)
    }

    async fn mcp_server_rotate_token(&self) -> RpcResult<()> {
        mcp_server::rotate_token(&self.rt).await.map_err(stub_err)
    }

    async fn mcp_server_audit(&self, limit: u32) -> RpcResult<Vec<mcp_server::AuditLine>> {
        mcp_server::audit_tail(&self.rt, limit)
            .await
            .map_err(stub_err)
    }

    async fn mcp_server_call(
        &self,
        tool: String,
        args: serde_json::Value,
        client: String,
    ) -> RpcResult<mcp_server::McpCallResult> {
        mcp_server::call(&self.rt, tool, args, client)
            .await
            .map_err(|e| {
                ErrorObjectOwned::owned(
                    jsonrpsee::types::error::INVALID_PARAMS_CODE,
                    format!("{e:#}"),
                    None::<()>,
                )
            })
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

    async fn mesh_remove(&self, id: String, reset_devices: Option<bool>) -> RpcResult<()> {
        let reset_devices = reset_devices == Some(true);
        send(
            &mesh_actor()?,
            mesh::Unpair {
                id: mesh::EngineId(id.clone()),
                reset_devices,
            },
        )
        .await?;
        if !reset_devices {
            return Ok(());
        }
        // D11: the unpair's rotation stores the new vault key with `device_reset_pending`; the
        // device key follows, as a new `mesh/device_key` version sealed under that key, so it
        // replicates to the remaining machines only. A crash before here is retried by the
        // vault's own tick (183 R1).
        let failed = |what: String| {
            ErrorObjectOwned::owned(
                ASK_FAILED,
                format!("mesh_remove: unpaired, but phone and iPad pairing was not reset: {what}"),
                None::<()>,
            )
        };
        let vault = send(&secrets()?, secrets::Handle).await?;
        match vault.rotation() {
            Some(r) if r.phase != secrets::RotationPhase::Failed && r.removed.contains(&id) => {}
            r => {
                return Err(failed(
                    r.and_then(|r| r.error)
                        .unwrap_or_else(|| "the vault key did not rotate".into()),
                ));
            }
        }
        let reset = vault
            .finish_device_reset()
            .await
            .map_err(|e| failed(format!("{e:#}")))?;
        if reset && let Ok(l) = link() {
            send(&l, link::ResetDevices).await?;
        }
        Ok(())
    }

    async fn mesh_set_hub(&self, pref: mesh::HubPref) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::SetHubPref(pref)).await
    }

    // 183 Phase 2 (§10.1): one `ask` each; the mesh answers "not built yet" where
    // the Nearby and rotation workers have not landed.

    async fn mesh_nearby(&self) -> RpcResult<Vec<mesh::NearbyEngine>> {
        send(&mesh_actor()?, mesh::Nearby).await
    }

    async fn mesh_nearby_watch(&self, open: bool) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::NearbyWatch(open)).await
    }

    async fn mesh_pair_nearby(&self, id: String) -> RpcResult<mesh::PairCeremony> {
        send(&mesh_actor()?, mesh::PairNearby(mesh::EngineId(id))).await
    }

    async fn mesh_pair_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::PairAnswer { ceremony, accept }).await
    }

    async fn mesh_confirm_peer(&self, id: String, keep: bool) -> RpcResult<()> {
        send(
            &mesh_actor()?,
            mesh::ConfirmPeer {
                id: mesh::EngineId(id),
                keep,
            },
        )
        .await
    }

    async fn mesh_set_messaging(&self, id: String, on: bool) -> RpcResult<()> {
        send(
            &mesh_actor()?,
            mesh::SetMessaging {
                id: mesh::EngineId(id),
                on,
            },
        )
        .await
    }

    async fn mesh_set_nearby(&self, on: bool) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::SetNearby(on)).await
    }

    async fn mesh_rename(&self, id: String, name: String) -> RpcResult<()> {
        send(
            &mesh_actor()?,
            mesh::Rename {
                id: mesh::EngineId(id),
                name,
            },
        )
        .await
    }

    async fn mesh_unpair_preview(&self, id: String) -> RpcResult<mesh::UnpairPreview> {
        let mut p = send(&mesh_actor()?, mesh::PreviewUnpair(mesh::EngineId(id))).await?;
        // W-37: the paired phones and iPads; none when link is not running here.
        if let Ok(l) = link() {
            let devices = send(&l, link::DevicesList).await?;
            p.devices = u32::try_from(devices.iter().filter(|d| !d.revoked).count()).unwrap_or(0);
        }
        Ok(p)
    }

    async fn vault_rotation(&self) -> RpcResult<Option<secrets::RotationState>> {
        send(&secrets()?, secrets::GetRotation).await
    }

    async fn vault_rotate(&self) -> RpcResult<secrets::RotationState> {
        send(&mesh_actor()?, mesh::RotateVault).await
    }

    // 183 Phase 3 (§10.1): each asks the A2A actor, which runs whatever `a2a.enabled` says
    // (W-26). Only a process without the a2a plugin answers the `off` values.

    async fn contact_window(&self, open: bool) -> RpcResult<Option<a2a::contacts::ContactWindow>> {
        let off = if open {
            Err(failed(a2a::contacts::OFF))
        } else {
            Ok(None)
        };
        contacts(&self.rt, a2a::contacts::ContactWindowOpen(open), off).await
    }

    async fn contact_nearby(&self) -> RpcResult<Vec<a2a::contacts::NearbyAgent>> {
        contacts(&self.rt, a2a::contacts::ContactsNearby, Ok(Vec::new())).await
    }

    async fn contact_pair(
        &self,
        target: a2a::contacts::ContactTarget,
    ) -> RpcResult<mesh::PairCeremony> {
        let off = Err(failed(a2a::contacts::OFF));
        contacts(&self.rt, a2a::contacts::ContactPair(target), off).await
    }

    async fn contact_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()> {
        let msg = a2a::contacts::ContactAnswer { ceremony, accept };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_list(&self) -> RpcResult<Vec<a2a::contacts::Contact>> {
        contacts(&self.rt, a2a::contacts::ContactsList, Ok(Vec::new())).await
    }

    async fn contact_update(
        &self,
        name: String,
        patch: a2a::contacts::ContactPatch,
    ) -> RpcResult<a2a::contacts::Contact> {
        let msg = a2a::contacts::ContactUpdate { name, patch };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_remove(&self, name: String, block: bool) -> RpcResult<()> {
        let msg = a2a::contacts::ContactRemove { name, block };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn a2a_exports(&self) -> RpcResult<Vec<a2a::contacts::Export>> {
        contacts(&self.rt, a2a::contacts::ExportsList, Ok(Vec::new())).await
    }

    async fn a2a_export_put(&self, export: a2a::contacts::Export) -> RpcResult<()> {
        let msg = a2a::contacts::ExportPut(export);
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn a2a_export_remove(&self, name: String) -> RpcResult<()> {
        let msg = a2a::contacts::ExportRemove { name };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn a2a_grantable_tools(&self) -> RpcResult<Vec<tools::GrantableTool>> {
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, tools::ContactGrantable).await
    }

    async fn contact_first_answer(
        &self,
        contact: String,
        export: String,
        answer: a2a::config::FirstContact,
    ) -> RpcResult<()> {
        let msg = a2a::contacts::ContactFirstAnswer {
            contact,
            export,
            answer,
        };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_first_waiting(&self) -> RpcResult<Vec<a2a::contacts::FirstWaiting>> {
        contacts(&self.rt, a2a::contacts::ContactsWaiting, Ok(Vec::new())).await
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

    async fn task_move_home(
        &self,
        ext: &Extensions,
        id: String,
        to: String,
    ) -> RpcResult<tasks::Task> {
        let task = send(&tasks_actor()?, tasks::GetTask(tasks::TaskId(id.clone()))).await?;
        let actor = crate::Client::actor(ext);
        let m = |check_only, session_moved| tasks::MoveHome {
            id: tasks::TaskId(id.clone()),
            to: to.clone(),
            actor: actor.clone(),
            session: None,
            check_only,
            session_moved,
        };
        // 182 S4: the move is checked before anything moves — a refused move
        // never leaves its session on the other machine.
        send(&tasks_actor()?, m(true, false)).await?;
        // 182 D10: a live work session of its checkout, owned here, moves first (181) so
        // it continues there; its refusal (a paused tree, the target refusing) refuses
        // the task's move. A desk or harness session stays: it is this engine's.
        let mut session_moved = false;
        if let Some(c) = task.checkout.as_ref().filter(|c| !c.orphaned)
            && let Ok(session) = c.session.parse::<SessionId>()
        {
            let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
            let me = store.engine().to_owned();
            let owned_here = to != me
                && matches!(store.fence(session).await, Ok(Some((_, owner))) if owner == me);
            let movable = owned_here
                && store
                    .session(session)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|s| s.desk.is_none() && s.agent.is_none());
            if movable {
                self.session_move(ext, session, to.clone()).await?;
                session_moved = true;
            }
        }
        send(&tasks_actor()?, m(false, session_moved)).await
    }

    async fn task_move_home_waiting(
        &self,
        ext: &Extensions,
        from: String,
    ) -> RpcResult<tasks::MovedHome> {
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let m = tasks::MoveWaiting {
            from,
            to: store.engine().to_owned(),
            actor: crate::Client::actor(ext),
        };
        send(&tasks_actor()?, m).await
    }

    async fn engine_agents_list(&self) -> RpcResult<Vec<roster::EngineAgent>> {
        send(&roster_actor()?, roster::ListEngineAgents).await
    }

    async fn activity_list(
        &self,
        filter: tasks::activity::ActivityFilter,
    ) -> RpcResult<Vec<tasks::activity::ActivityEntry>> {
        let actor = lookup::<tasks::activity::Activity>(tasks::activity::ACTIVITY_ACTOR)?;
        send(&actor, tasks::activity::ListActivity(filter)).await
    }

    async fn attention_list(
        &self,
        ext: &Extensions,
    ) -> RpcResult<Vec<roster::attention::AttentionItem>> {
        let mut items: Vec<roster::attention::AttentionItem> =
            send(&attention_actor()?, roster::attention::ListAttention).await?;
        if let Some(peer) = ext.get::<link::RpcPeer>() {
            let focus = self.focus.read().get(&peer.id()).cloned();
            let mut seen = self.seen.write();
            let fresh = !seen.contains_key(&peer.id());
            mask_alerts(
                &mut items,
                focus.as_deref(),
                seen.entry(peer.id()).or_default(),
            );
            drop(seen);
            if fresh {
                self.forget_on_close(peer.clone());
            }
        }
        Ok(items)
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
        self.authorize_fwd(ext, session).await?;
        // 139 §9.2 (D18): an action, so it runs on the owner (I1). 183 §4.1:
        // every session's press forwards to a connected owner (`-32010` when
        // it is not); here it resumes this engine's own open turn. Idempotent
        // against W2's own resume-on-take: a turn already in flight answers
        // "resumed lazily".
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
                cause: None,
                from: None,
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

    async fn export_artifact(
        &self,
        ext: &Extensions,
        id: String,
        dest: PathBuf,
        session: Option<SessionId>,
    ) -> RpcResult<PathBuf> {
        self.authorize_opt(ext, session).await?;
        send(&artifact()?, artifact::Export { id, dest, session }).await
    }

    async fn get_app(
        &self,
        ext: &Extensions,
        record_call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<AppDoc> {
        let apps = apps()?;
        self.authorize_opt(ext, session).await?;
        // The app plugin reads any store by call id: the record must be one this caller may see.
        send(
            &artifact()?,
            artifact::search::Get {
                call_id: record_call_id.clone(),
                session,
            },
        )
        .await?;
        send(&apps, GetApp { record_call_id }).await
    }

    async fn publish_app(
        &self,
        ext: &Extensions,
        session: SessionId,
        name: String,
        source: String,
    ) -> RpcResult<AppDoc> {
        self.authorize(ext, session).await?;
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

    async fn get_artifact(
        &self,
        ext: &Extensions,
        call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<artifact::Artifact> {
        self.authorize_opt(ext, session).await?;
        match send(
            &artifact()?,
            artifact::search::Get {
                call_id: call_id.clone(),
                session,
            },
        )
        .await
        {
            Ok(a) => Ok(a),
            // 68 §7: the index is engine-local; a record shown elsewhere comes from a peer
            // (peers serve durable records only).
            Err(e) => artifact::mesh::fetch_record(&self.rt.data_dir, &call_id, None)
                .await
                .map_err(|_| e),
        }
    }

    async fn get_artifact_listing(
        &self,
        ext: &Extensions,
        call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<artifact::archive::ArchiveListing> {
        self.authorize_opt(ext, session).await?;
        let a = send(&artifact()?, artifact::search::Get { call_id, session }).await?;
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
        // 183 §5.1: a focused session streams from its owner while focused. The
        // relay skips one owned here and follows it if it moves away; a session
        // leaving the focus drops its guard (the stream closes).
        {
            let mut follows = self.follows.lock();
            let held = follows.entry(peer.id()).or_default();
            held.retain(|s, _| sessions.contains(s));
            for s in &sessions {
                if !held.contains_key(s)
                    && let Some(guard) = sessionsync::events::follow(*s)
                {
                    held.insert(*s, guard);
                }
            }
        }
        let fresh = self.focus.write().insert(peer.id(), set.clone()).is_none();
        if fresh {
            self.forget_on_close(peer.clone());
        }
        // 155 §7.5: looking at a session marks its finished alert seen here.
        if !set.is_empty()
            && let Ok(actor) = attention_actor()
            && let Ok(mut items) = actor.ask(roster::attention::ListAttention).await
        {
            let mut seen = self.seen.write();
            let fresh = !seen.contains_key(&peer.id());
            mask_alerts(&mut items, Some(&set), seen.entry(peer.id()).or_default());
            drop(seen);
            if fresh {
                self.forget_on_close(peer);
            }
        }
        Ok(())
    }

    fn ping(&self) -> RpcResult<()> {
        Ok(())
    }

    fn client_log(
        &self,
        ext: &Extensions,
        level: String,
        source: String,
        message: String,
    ) -> RpcResult<()> {
        let conn = ext.get::<crate::ConnId>().map_or(0, |c| c.0);
        crate::diag::client_log(&peer(ext)?, conn, &level, &source, &message);
        Ok(())
    }
}

/// 155 §7.3 / §7.5: blank `alert` for this connection — every item whose
/// session it focuses (a focused `finished` alert is marked seen first) and
/// every `finished` alert it has already seen.
fn mask_alerts(
    items: &mut [roster::attention::AttentionItem],
    focus: Option<&HashSet<String>>,
    seen: &mut HashSet<String>,
) {
    for it in items {
        let Some(alert) = it.alert.as_ref() else {
            continue;
        };
        let focused = it
            .session
            .as_deref()
            .zip(focus)
            .is_some_and(|(s, f)| f.contains(s));
        let finished = alert.class == roster::alert::AlertClass::Finished;
        if focused && finished {
            seen.insert(alert.id.clone());
        }
        if focused || (finished && seen.contains(&alert.id)) {
            it.alert = None;
        }
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
/// owner; a `SubagentSpawned` (or 141 `ObservedChildChanged`) under a private
/// parent enrols `child` first so its own later events are filtered too.
/// Events naming no known member pass: durable trees are not tracked.
fn admitted(ev: &serde_json::Value, peer: u64, private: &PrivateMap) -> bool {
    let id = |k: &str| {
        ev.get(k)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<SessionId>().ok())
    };
    if announces_child(ev)
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

/// `SubagentSpawned` or 141 `ObservedChildChanged`: the two events that
/// introduce a `child` under a `parent` (the latter repeats on every change,
/// which the enrolments below tolerate).
fn announces_child(ev: &serde_json::Value) -> bool {
    matches!(
        ev.get("type").and_then(|t| t.as_str()),
        Some("SubagentSpawned" | "ObservedChildChanged")
    )
}

/// Does a subscription on `session` forward `ev`? With `tree` (descendants
/// mode) an event passes when its `session` is in the tree, or it is a
/// `SubagentSpawned` / `ObservedChildChanged` whose `parent` is — then
/// `child` joins the tree. Without, everything passes unless both sides name
/// a different session. `CommunicationChanged` always passes: a global or
/// ancestor switch changes every descendant's effective state, the filter
/// cannot see ancestry, and the event carries only a session id and a
/// revision (50 §10).
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
    if announces_child(ev)
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
                from: None,
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

    /// 140 §12.5: every `autoresearch.*` method is registered under its dotted name.
    #[tokio::test]
    async fn autoresearch_methods_are_registered() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let names: Vec<&str> = Server::new(rt).into_rpc().method_names().collect();
        for m in autoresearch::rpc_impl::METHODS {
            assert!(names.contains(&m), "{m} missing from {names:?}");
        }
    }

    /// 170 D1: `slack_manifest` answers `slack::MANIFEST` verbatim with no Slack
    /// actor running (unconfigured), and the text is the manifest the setup uses.
    #[tokio::test]
    async fn slack_manifest_answers_the_compiled_manifest_without_slack() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
        let req = r#"{"jsonrpc":"2.0","method":"slack_manifest","params":[],"id":1}"#;
        let (resp, _) = methods.raw_json_request(req, 1).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        assert_eq!(v["result"].as_str(), Some(slack::MANIFEST), "{v}");
        let m: serde_yaml::Value = serde_yaml::from_str(slack::MANIFEST).unwrap();
        assert_eq!(m["features"]["slash_commands"][0]["command"], "/theuth");
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

    /// 185 R1 A5: the hub side of a forwarded Dream now — the positional
    /// `[RunRequest]` the forwarder sends dispatches into dreaming (no actor
    /// here → `NOT_RUNNING`), never `INVALID_PARAMS`, never a second hop.
    #[tokio::test]
    async fn a_forwarded_dream_now_runs_on_the_hub_with_the_forwarders_params() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
        methods.extensions_mut().insert(link::RpcPeer::internal());
        methods
            .extensions_mut()
            .insert(forward::Forwarded(replica::PeerId("node".into())));
        let request = dreaming::RunRequest {
            budget_usd: 2.0,
            mode: dreaming::RunMode::Pass,
            run_timeout_secs: None,
        };
        let params = serde_json::to_string(&[&request]).unwrap();
        let req =
            format!(r#"{{"jsonrpc":"2.0","method":"dreaming.run_now","params":{params},"id":1}}"#);
        let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        assert_eq!(
            v["error"]["code"].as_i64(),
            Some(i64::from(NOT_RUNNING)),
            "{v}"
        );
    }

    /// 185 R1 A5: a hub that refuses or does not answer is named as the hub;
    /// the code and the `OwnerUnreachable` payload stay for the UI.
    #[test]
    fn the_hubs_refusal_of_dream_now_is_said_as_the_hubs() {
        let off = hub_said(ErrorObjectOwned::owned(
            ASK_FAILED,
            "dreaming is off in config (dreaming.enabled: false)",
            None::<()>,
        ));
        assert_eq!(off.code(), ASK_FAILED);
        assert_eq!(
            off.message(),
            "Dream now runs on the hub, which said: dreaming is off in config (dreaming.enabled: false)"
        );
        let gone = hub_said(ErrorObjectOwned::owned(
            forward::OWNER_UNREACHABLE,
            "no answer within 30s",
            Some(forward::FrontDoorError::OwnerUnreachable {
                owner: "hub".into(),
                last_seen: None,
            }),
        ));
        assert_eq!(gone.code(), forward::OWNER_UNREACHABLE);
        assert!(
            gone.message()
                .starts_with("Dream now runs on the hub, which did not answer"),
            "{}",
            gone.message()
        );
        assert!(
            gone.data()
                .is_some_and(|d| d.get().contains("OwnerUnreachable"))
        );
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

    /// 141 §8.2: `ObservedChildChanged` is the watch-only twin of
    /// `SubagentSpawned` for both filters — it enrols `child` in a private
    /// tree and in a `descendants` subscription, so the child's transcript
    /// events follow; a repeat (every state change re-announces) is harmless.
    #[test]
    fn observed_child_changed_enrols_like_subagent_spawned() {
        let (p, c, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        let announce = serde_json::json!({
            "type": "ObservedChildChanged", "parent": p, "child": c, "name": "ReadA",
            "model": null, "observed": { "source": "omp_task", "external": "ReadA", "state": "running" }
        });
        let private: PrivateMap = Default::default();
        private.write().insert(p, (p, 7));
        assert!(!admitted(&announce, 8, &private));
        assert!(admitted(&announce, 7, &private));
        assert_eq!(private.read().get(&c), Some(&(p, 7)));
        let child = serde_json::json!({ "type": "TextDelta", "session": c, "text": "hi" });
        assert!(!admitted(&child, 8, &private));

        let mut tree: HashSet<String> = HashSet::from([p.to_string()]);
        assert!(!wanted(&child, Some(&p.to_string()), Some(&mut tree)));
        assert!(wanted(&announce, Some(&p.to_string()), Some(&mut tree)));
        assert!(
            wanted(&announce, Some(&p.to_string()), Some(&mut tree)),
            "repeat"
        );
        assert!(wanted(&child, Some(&p.to_string()), Some(&mut tree)));
        let stranger = serde_json::json!({ "type": "ObservedChildChanged", "parent": other, "child": SessionId::new() });
        assert!(!wanted(&stranger, Some(&p.to_string()), Some(&mut tree)));
        assert_eq!(tree.len(), 2);
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

    /// 160 §3.2/§4/§5: the four methods are registered, `cost_calls` takes a
    /// partial query (absent filters = none), `usage_limits` answers from the
    /// hub, and the wire shapes the UI codes against hold.
    #[tokio::test]
    async fn cost_methods_are_registered_with_their_wire_shapes() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
        let call = |body: String| {
            let methods = methods.clone();
            async move {
                let (resp, _) = methods.raw_json_request(&body, 1).await.unwrap();
                serde_json::from_str::<serde_json::Value>(resp.get()).unwrap()
            }
        };
        for (m, params) in [
            ("cost_cube", "[null]"),
            ("cost_calls", r#"[{"bases":["reported"],"limit":5}]"#),
            ("usage_limits", "[]"),
            ("usage_limits_refresh", "[null]"),
        ] {
            let v = call(format!(
                r#"{{"jsonrpc":"2.0","method":"{m}","params":{params},"id":1}}"#
            ))
            .await;
            let code = v["error"]["code"].as_i64();
            assert!(
                code.is_none() || code == Some(i64::from(NOT_RUNNING)),
                "{m}: {v}"
            );
        }
        let v =
            call(r#"{"jsonrpc":"2.0","method":"usage_limits","params":[],"id":1}"#.into()).await;
        assert!(v["result"]["providers"].is_array(), "{v}");
        let ev = serde_json::to_value(agent::CostCubeChanged {
            epoch: "e".into(),
            rev: 3,
        })
        .unwrap();
        assert_eq!(
            ev,
            serde_json::json!({"type": "CostCubeChanged", "epoch": "e", "rev": 3})
        );
        let q: agent::CostCallsQuery =
            serde_json::from_str(r#"{"bases":["unpriced"],"before":{"at_ms":5,"id":"x"}}"#)
                .unwrap();
        assert_eq!(q.bases, Some(vec![agent::CostBasis::Unpriced]));
        assert_eq!(q.limit, None);
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
            stores: Arc<artifact::Stores>,
            methods: jsonrpsee::Methods,
            events: mpsc::UnboundedReceiver<(bool, PreviewId)>,
            _dir: tempfile::TempDir,
        }

        impl Harness {
            async fn new() -> Self {
                let dir = tempfile::tempdir().unwrap();
                let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
                // Paused time auto-advances while sqlx waits on its worker thread and
                // trips the pool's acquire timeout: open the store on real time.
                tokio::time::resume();
                let state = artifact::State::open(dir.path(), rt.clone(), Vec::new())
                    .await
                    .unwrap();
                tokio::time::pause();
                let stores = Arc::new(artifact::Stores::new(Arc::new(state)));
                let methods: jsonrpsee::Methods = Server::new(rt.clone()).into_rpc().into();
                let (tx, events) = mpsc::unbounded_channel();
                let cap = Capture::spawn(Capture(tx));
                rt.subscribe::<Capture, PagePreviewRequested>(&cap).await;
                rt.subscribe::<Capture, PagePreviewResolved>(&cap).await;
                Self {
                    rt,
                    stores,
                    methods,
                    events,
                    _dir: dir,
                }
            }

            /// Run the tool for `session` on its own task; the width defaults.
            fn call(&self, session: SessionId) -> tokio::task::JoinHandle<ToolOutput> {
                let tool = PagePreviewTool {
                    rt: self.rt.clone(),
                    stores: self.stores.clone(),
                };
                let cx = ToolCx {
                    session,
                    call_id: "c".into(),
                    quiet: false,
                    resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
                    cancel: tools::CancellationToken::new(),
                    promote_after: None,
                };
                tokio::spawn(async move {
                    Tool::call(
                        &tool,
                        PreviewArgs {
                            html: Some("<p>x</p>".into()),
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
        async fn two_previews_in_one_session_run_in_parallel() {
            let mut h = Harness::new().await;
            let session = SessionId::new();
            let first = h.call(session);
            let id = h.requested().await;
            let second = h.call(session);
            let other_id = h.requested().await;
            assert_ne!(other_id, id);
            assert!(h.answer(id, &done(100)).await);
            assert!(h.answer(other_id, &done(200)).await);
            assert!(!first.await.unwrap().is_error);
            assert!(!second.await.unwrap().is_error);
        }
    }

    /// 164 D3: the fence snapshot round trip against the real `answer_fence_snapshot`
    /// dispatch, and a full-size answer over a real authenticated WebSocket.
    mod fence_snapshot {
        use super::*;

        use artifact::snapshot::{
            Appearance, Client, FenceLang, FenceSnapshotAnswer, FenceSnapshotId,
            FenceSnapshotRequested, FenceSnapshotResolved, SnapshotOutcome,
        };
        use kameo::actor::Spawn;
        use kameo::message::Context;
        use tokio::sync::{broadcast, mpsc};

        /// Bus subscriber: `(resolved, id)` of every request and resolution, in order.
        #[derive(kameo::Actor)]
        struct Capture(mpsc::UnboundedSender<(bool, FenceSnapshotId)>);

        impl Message<FenceSnapshotRequested> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: FenceSnapshotRequested, _: &mut Context<Self, ()>) {
                let _ = self.0.send((false, m.id));
            }
        }

        impl Message<FenceSnapshotResolved> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: FenceSnapshotResolved, _: &mut Context<Self, ()>) {
                let _ = self.0.send((true, m.id));
            }
        }

        /// A runtime with a capture on its bus and a live event feed (no subscriber at all
        /// short-circuits to "no renderer").
        async fn harness() -> (Runtime, mpsc::UnboundedReceiver<(bool, FenceSnapshotId)>) {
            let (tx, rx) = broadcast::channel::<crate::bridge::Event>(4);
            artifact::preview::watch_feed(tx);
            std::mem::forget(rx);
            if let Ok(Some(b)) =
                ActorRef::<crate::bridge::Bridge>::lookup(crate::bridge::BRIDGE_ACTOR)
                && let Ok(tx) = b.ask(crate::bridge::Feed).await
            {
                std::mem::forget(tx.subscribe());
            }
            let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
            let (tx, events) = mpsc::unbounded_channel();
            let cap = Capture::spawn(Capture(tx));
            rt.subscribe::<Capture, FenceSnapshotRequested>(&cap).await;
            rt.subscribe::<Capture, FenceSnapshotResolved>(&cap).await;
            (rt, events)
        }

        fn render(rt: &Runtime, source: &str) -> tokio::task::JoinHandle<SnapshotOutcome> {
            let (rt, source) = (rt.clone(), source.to_owned());
            tokio::spawn(async move {
                artifact::snapshot::render(
                    &rt,
                    FenceLang::JsonRender,
                    &source,
                    560,
                    Appearance::Light,
                    2,
                )
                .await
            })
        }

        /// A PNG header `w`×`h` padded with zeros to `len` bytes.
        fn png(w: u32, h: u32, len: usize) -> Vec<u8> {
            let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
            b.extend(b"IHDR");
            b.extend(w.to_be_bytes());
            b.extend(h.to_be_bytes());
            b.resize(len.max(24), 0);
            b
        }

        fn done(bytes: &[u8]) -> FenceSnapshotAnswer {
            FenceSnapshotAnswer::Done {
                png: base64::engine::general_purpose::STANDARD.encode(bytes),
                width: 1,
                height: 1,
                client: Client::Desktop,
            }
        }

        /// The raw `answer_fence_snapshot` result.
        async fn answer(
            methods: &jsonrpsee::Methods,
            id: FenceSnapshotId,
            answer: &FenceSnapshotAnswer,
        ) -> bool {
            let req = format!(
                r#"{{"jsonrpc":"2.0","method":"answer_fence_snapshot","params":[{},{}],"id":1}}"#,
                serde_json::to_string(&id).unwrap(),
                serde_json::to_string(answer).unwrap()
            );
            let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
            let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
            v["result"].as_bool().unwrap_or_else(|| panic!("{v}"))
        }

        #[tokio::test(start_paused = true)]
        async fn first_done_wins_and_late_answers_are_refused() {
            let (rt, mut events) = harness().await;
            let methods: jsonrpsee::Methods = Server::new(rt.clone()).into_rpc().into();
            let call = render(&rt, "{\"a\":1}");
            let (resolved, id) = events.recv().await.unwrap();
            assert!(!resolved);
            let started = FenceSnapshotAnswer::Started {
                client: Client::Web,
            };
            assert!(answer(&methods, id, &started).await);
            let pic = png(1120, 600, 64);
            assert!(answer(&methods, id, &done(&pic)).await);
            assert!(
                !answer(&methods, id, &done(&pic)).await,
                "second Done is late"
            );
            assert!(
                !answer(&methods, id, &started).await,
                "Started after Done is late"
            );
            assert_eq!(
                call.await.unwrap(),
                SnapshotOutcome::Done {
                    png: pic,
                    width: 1120,
                    height: 600,
                    client: Client::Desktop
                }
            );
            assert_eq!(events.recv().await.unwrap(), (true, id));
        }

        /// The largest PNG a client sends (4 MiB → ~5.6 MB of base64, ~6 MB with the
        /// envelope) fits the server's request limit (jsonrpsee's default 10 MiB) over a
        /// real authenticated WebSocket.
        #[tokio::test]
        async fn a_four_mib_png_passes_the_websocket() {
            use jsonrpsee::core::client::ClientT;
            use jsonrpsee::rpc_params;
            use jsonrpsee::ws_client::{HeaderMap, HeaderValue, WsClientBuilder};

            let (rt, mut events) = harness().await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let methods: jsonrpsee::Methods = Server::new(rt.clone()).into_rpc().into();
            tokio::spawn(crate::accept(
                listener,
                methods,
                "t".into(),
                std::future::pending(),
            ));
            let mut headers = HeaderMap::new();
            headers.insert("authorization", HeaderValue::from_static("Bearer t"));
            let client = WsClientBuilder::default()
                .set_headers(headers)
                .build(format!("ws://{addr}"))
                .await
                .unwrap();

            let call = render(&rt, "{\"big\":true}");
            let (_, id) = events.recv().await.unwrap();
            let pic = png(4000, 8000, artifact::snapshot::MAX_PNG);
            let answer = done(&pic);
            let body = serde_json::to_string(&answer).unwrap().len();
            assert!(body > 5_500_000, "{body}");
            let ok: bool = client
                .request("answer_fence_snapshot", rpc_params![id, answer])
                .await
                .unwrap();
            assert!(ok);
            match call.await.unwrap() {
                SnapshotOutcome::Done { png, height, .. } => {
                    assert_eq!((png.len(), height), (artifact::snapshot::MAX_PNG, 8000));
                }
                other => panic!("{other:?}"),
            }
        }
    }

    fn link_status(enabled: bool, harness: Option<&str>, mesh: Option<&str>) -> LinkStatus {
        LinkStatus {
            enabled,
            relay: "wss://relay.example/".into(),
            name: "mac".into(),
            harness_id: harness.map(Into::into),
            connected: enabled,
            devices: vec![],
            pairing: None,
            mesh: mesh.map(|m| link::MeshLink {
                device_id: m.into(),
                connected: false,
            }),
        }
    }

    /// 184 §7.1, §7.2, W-j: v2 only from the home in a mesh; everywhere else today's v1 —
    /// including a lone engine with a paired phone (`1.<mesh device id>`); Link off in a mesh is
    /// the v1 mesh link, whose id Link never holds there.
    #[test]
    fn share_links_are_v2_on_the_home_only_and_v1_as_today_elsewhere() {
        use document::LinkVia::{Home, Mesh};
        let ids = |s: &LinkStatus, m: Option<&str>, here| link_ids(s, m, here).unwrap();
        let phone = link_status(true, Some("H"), Some("M"));
        assert_eq!(
            ids(&phone, None, false),
            ("1.M".into(), Mesh),
            "lone engine + phone"
        );
        let alone = link_status(true, Some("H"), None);
        assert_eq!(
            ids(&alone, None, false),
            ("1.H".into(), Home),
            "before any device"
        );
        assert_eq!(
            ids(&phone, Some("M"), true),
            ("2.H.M".into(), Home),
            "the home"
        );
        let follower = link_status(true, Some("H"), None);
        assert_eq!(
            ids(&follower, Some("M"), true),
            ("2.H.M".into(), Home),
            "MeshLinkId read"
        );
        assert_eq!(
            ids(&phone, Some("M"), false),
            ("1.M".into(), Mesh),
            "not the home"
        );
        let off = link_status(false, Some("H"), None);
        assert_eq!(
            ids(&off, Some("M"), true),
            ("1.M".into(), Mesh),
            "Link off: a mesh link"
        );
        assert_eq!(
            link_ids(&off, None, true).unwrap_err().message(),
            link::MESH_LINK_UNAVAILABLE
        );
        assert_eq!(
            link_ids(&link_status(true, None, None), None, false)
                .unwrap_err()
                .message(),
            "link has no identity yet"
        );
        assert_eq!(link_origin(&phone.relay), "https://relay.example");
        assert_eq!(link_origin("ws://127.0.0.1:9/"), "http://127.0.0.1:9");
    }

    /// 184 §10.4: a `busy` join (a hand-back in flight) is retried by the page and never counts
    /// toward the guest's three refusals; any other refusal does.
    #[test]
    fn a_busy_join_is_not_a_refusal() {
        let (peer, _guard) = link::RpcPeer::open_guest([7; 32], [9; 32]);
        let guest = peer.guest().unwrap();
        // Z184-R2: a move in flight or a transient failure never counts toward the close.
        for e in [
            document::share::BUSY,
            "hub_moved",
            "server_moved",
            "NotHub:abc",
        ] {
            join_outcome::<()>(guest, &Err(failed(e)));
        }
        assert_eq!(guest.refusals(), 0);
        join_outcome::<()>(guest, &Err(failed("bad_proof")));
        join_outcome::<()>(guest, &Err(failed("not_member")));
        assert_eq!(guest.refusals(), 2);
        // Z184-R3: the join's own refusals count too.
        join_outcome::<()>(guest, &Err(failed("link_revoked")));
        join_outcome::<()>(guest, &Err(failed("member_revoked")));
        assert_eq!(guest.refusals(), 4);
    }

    /// 184 §12.4: Move to's refusals name the machine, or an unknown id's first 8 characters.
    #[test]
    fn move_refusals_name_the_machine() {
        let id = "0123456789abcdef0123";
        assert_eq!(
            named(&format!("{id} is offline"), id, Some("xeus")),
            "xeus is offline"
        );
        assert_eq!(
            named(&format!("already lives on {id}"), id, None),
            "already lives on 01234567"
        );
        assert_eq!(
            named(
                &format!("moved to {id}; it has not taken the note yet"),
                id,
                Some("C")
            ),
            "moved to C; it has not taken the note yet"
        );
    }
}
