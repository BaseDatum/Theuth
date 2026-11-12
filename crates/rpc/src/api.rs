//! The JSON-RPC surface. Every method is a lazy registry lookup plus one
//! `ask`; the subscription drains a `broadcast::Receiver` fed by
//! [`crate::bridge`].

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;


use ::search::SearchCategory;
use a2a::{A2A_ACTOR, A2aActor, A2aStatus};
use agent::SetModelFavorite;
use agent::context::save_blob;
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
    LoadSkill, Loaded, Login, LoginInfo, Logout, ModelInfo, PeerMessagePage,
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
    CloseResult, CreateSession, CreateSessionOptions, LeaveSession,
    ListProjectStyles, PeerDeliver, PeerEnvelope, PrivateTrees, ProjectStyle, ResumePrivate,
    SessionCost, SessionCostQuery, SessionOpen, SetProjectStyle, SetSessionProject, SpendReport,
    SpendReportQuery,
};
use agent::{AwsProfiles, ListAwsProfiles, SetAwsProfile};
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
    CancelPair, Enable, LinkConfig, LinkStatus, Pair, Pairing, Rename,
    RenameDevice, Revoke,
};

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
    AskRequested, Decision, Gate, GetApprovals, GetPtyIdle, JOBS_ACTOR,
    Jobs, ListServers, ListTools, Mcp, McpHomeAction, McpLogin,
    McpLogout, McpProgress, McpServer, Progress, RemoveServer, RepoInfo, Resolve, SetApprovals,
    SetGate, SetHidden, SetMcpHome, SetPtyIdle, SyncMcp, ToolRegistry, ToolSpec,
};
use tools::{
    ListTerminals, PtyChunk, Retention, TerminalSummary,
};
use uuid::Uuid;
use voice::{SpeechId, VoiceStatus};

#[cfg(test)]
use rpc_core::{Event, FocusMap, BACKLOG};
use agent::rpc_forward as forward;
use crate::pathac::PathCompletion;
use crate::search::SearchResponse;
use rpc_core::{is_delta, Stream, Backlog, Next, ask, send, lookup, failed, peer, owner, dreaming_err, stub_err, ASK_FAILED, NOT_RUNNING, UNAUTHORIZED};
use agent::rpc_support::{owner_root, RpcServerExt};
use link::rpc_support::{link, mint_as_holder};
use mesh::rpc_support::mesh_actor;
use roster::rpc_support::{roster_actor, attention_actor, mask_alerts};
use schedule::rpc_support::schedule;
use secrets::rpc_support::secrets;

/// 191: `a2a_forward` from a machine `mesh.messaging_off` lists: the words the contact hears
/// (through the receiving machine), naming no machine.
const A2A_FORWARD_MESSAGING_OFF: &str = "export unavailable: messages are off between our machines";

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

/// The wire API is split into domain server and client traits.
/// [`into_rpc_module`] merges the server traits; [`clients`] exports the typed client methods
/// for the in-process [`crate::client::Local`] or a WebSocket (63 §2.3).
mod hub;
mod sessions;
mod turns;
mod settings;
mod tooling;
mod skills;
mod screens;
mod mcp;
mod concepts;
mod documents;
mod notebooks;
mod codemap;
mod messaging;
mod network;
mod routines;
mod boards;
mod artifacts;
mod credentials;
mod speech;
mod meetings;
mod kernels;
mod dreams;
mod research;
mod sandboxes;

pub use hub::{CoreApiClient, CoreApiServer};
pub use sessions::{SessionsApiClient, SessionsApiServer};
pub use turns::{TurnsApiClient, TurnsApiServer};
pub use settings::{SettingsApiClient, SettingsApiServer};
pub use tooling::{SESSION_TOOLS, ToolingApiClient, ToolingApiServer};
pub use skills::{SkillsApiClient, SkillsApiServer};
pub use screens::{ScreensApiClient, ScreensApiServer};
pub use mcp::{McpApiClient, McpApiServer};
pub use concepts::{ConceptsApiClient, ConceptsApiServer};
pub use documents::{DocumentsApiClient, DocumentsApiServer};
pub use notebooks::{NotebooksApiClient, NotebooksApiServer};
pub use codemap::{CodemapApiClient, CodemapApiServer};
pub use messaging::{MessagingApiClient, MessagingApiServer};
pub use network::{NetworkApiClient, NetworkApiServer};
pub use routines::{RoutinesApiClient, RoutinesApiServer};
pub use boards::{BoardsApiClient, BoardsApiServer};
pub use artifacts::{ArtifactsApiClient, ArtifactsApiServer};
pub use credentials::{CredentialsApiClient, CredentialsApiServer};
pub use speech::{SpeechApiClient, SpeechApiServer};
pub use meetings::{MeetingsApiClient, MeetingsApiServer};
pub use kernels::{KernelsApiClient, KernelsApiServer};
pub use dreams::{DreamsApiClient, DreamsApiServer};
pub use research::{ResearchApiClient, ResearchApiServer};
pub use sandboxes::{SandboxesApiClient, SandboxesApiServer};

/// All domain client traits for typed methods on any jsonrpsee client.
pub mod clients {
    pub use super::hub::CoreApiClient;
    pub use super::sessions::SessionsApiClient;
    pub use super::turns::TurnsApiClient;
    pub use super::settings::SettingsApiClient;
    pub use super::tooling::ToolingApiClient;
    pub use super::skills::SkillsApiClient;
    pub use super::screens::ScreensApiClient;
    pub use super::mcp::McpApiClient;
    pub use super::concepts::ConceptsApiClient;
    pub use super::documents::DocumentsApiClient;
    pub use super::notebooks::NotebooksApiClient;
    pub use super::codemap::CodemapApiClient;
    pub use super::messaging::MessagingApiClient;
    pub use super::network::NetworkApiClient;
    pub use super::routines::RoutinesApiClient;
    pub use super::boards::BoardsApiClient;
    pub use super::artifacts::ArtifactsApiClient;
    pub use super::credentials::CredentialsApiClient;
    pub use super::speech::SpeechApiClient;
    pub use super::meetings::MeetingsApiClient;
    pub use super::kernels::KernelsApiClient;
    pub use super::dreams::DreamsApiClient;
    pub use super::research::ResearchApiClient;
    pub use super::sandboxes::SandboxesApiClient;
}

/// Merge every domain, rejecting duplicate wire method names.
pub fn into_rpc_module(server: Server) -> jsonrpsee::RpcModule<Server> {
    let mut module = CoreApiServer::into_rpc(server.clone());
    module.merge(SessionsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(TurnsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(SettingsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(agent::rpc_module(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(ToolingApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(SkillsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(ScreensApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(McpApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(ConceptsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(DocumentsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(NotebooksApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(CodemapApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(MessagingApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(NetworkApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(RoutinesApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(BoardsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(ArtifactsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(CredentialsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(SpeechApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(MeetingsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(KernelsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(DreamsApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(ResearchApiServer::into_rpc(server.clone()))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module.merge(SandboxesApiServer::into_rpc(server))
        .unwrap_or_else(|error| panic!("RPC module merge failed: {error}"));
    module
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

/// 188: the flags Settings had no write path for: four `<section>.enabled` and
/// the two `allow_private_network` keys behind Network's "Local network
/// access" (16 §5, amended 2026-11-11). Any other string fails to
/// deserialize, an `INVALID_PARAMS` error listing these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ConfigFlagKey {
    /// `decision.enabled` (57 §7.1): off, the decision actor escalates every ask.
    #[serde(rename = "decision.enabled")]
    Decision,
    /// `a2a.enabled` (183): the A2A HTTP client/server machinery.
    #[serde(rename = "a2a.enabled")]
    A2a,
    /// `computer.enabled` (27): the computer-use actor, tools and input thread.
    #[serde(rename = "computer.enabled")]
    Computer,
    /// `double.enabled` (126): the double actors, tools and prompt paragraph. Its own
    /// `double_settings_set` needs the running scheduler, so it cannot turn it on.
    #[serde(rename = "double.enabled")]
    Double,
    /// `web.allow_private_network`: the web tools reach the local network.
    #[serde(rename = "web.allow_private_network")]
    WebPrivateNetwork,
    /// `browser.allow_private_network`: the browser reaches the local network.
    #[serde(rename = "browser.allow_private_network")]
    BrowserPrivateNetwork,
}

impl ConfigFlagKey {
    /// The config.yaml `(section, key)` this flag is.
    fn path(self) -> (&'static str, &'static str) {
        match self {
            Self::Decision => ("decision", "enabled"),
            Self::A2a => ("a2a", "enabled"),
            Self::Computer => ("computer", "enabled"),
            Self::Double => ("double", "enabled"),
            Self::WebPrivateNetwork => ("web", "allow_private_network"),
            Self::BrowserPrivateNetwork => ("browser", "allow_private_network"),
        }
    }

    /// The flag in `config` as its plugin reads it at start: a missing or
    /// unreadable section is the section type's default.
    fn read(self, config: &figment::Figment) -> bool {
        let (s, _) = self.path();
        match self {
            Self::Decision => {
                let c: decision::DecisionConfig = config.extract_inner(s).unwrap_or_default();
                c.enabled
            }
            Self::A2a => {
                let c: a2a::config::A2aConfig = config.extract_inner(s).unwrap_or_default();
                c.enabled
            }
            Self::Computer => {
                let c: computer::ComputerConfig = config.extract_inner(s).unwrap_or_default();
                c.enabled
            }
            Self::Double => {
                let c: double::DoubleConfig = config.extract_inner(s).unwrap_or_default();
                c.enabled
            }
            Self::WebPrivateNetwork => {
                let c: web::WebConfig = config.extract_inner(s).unwrap_or_default();
                c.allow_private_network
            }
            Self::BrowserPrivateNetwork => {
                let c: browser::BrowserConfig = config.extract_inner(s).unwrap_or_default();
                c.allow_private_network
            }
        }
    }
}

/// 188: a [`ConfigFlagKey`] as config.yaml holds it and as this engine runs it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConfigFlag {
    /// Which flag.
    pub key: ConfigFlagKey,
    /// The value `<data_dir>/config.yaml` holds now (the section's default when absent).
    pub enabled: bool,
    /// The value this process started with (its boot config); plugins keep it until a restart.
    pub running: bool,
    /// `enabled != running`: the saved value applies after theuth restarts.
    pub restart_required: bool,
}

use rpc_core::{Server, PrivateMap};

















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



/// 191 (D8): each contact's `paired_on` engine id by that machine's name, looked up as
/// `session_move` does: this machine's `self_name`, a mesh peer's name, else the id's first
/// 8 characters (the machine left the mesh, or the mesh cannot answer). Display only.
async fn name_paired_on(contacts: &mut [a2a::contacts::Contact]) {
    if contacts.iter().all(|c| c.paired_on.is_none()) {
        return;
    }
    let status = match mesh_actor() {
        Ok(m) => m.ask(mesh::Status).await.ok(),
        Err(_) => None,
    };
    for id in contacts.iter_mut().filter_map(|c| c.paired_on.as_mut()) {
        let name = status.as_ref().and_then(|s| {
            if s.self_id.0 == *id {
                return Some(s.self_name.clone());
            }
            s.peers
                .iter()
                .find(|p| p.id.0 == *id)
                .map(|p| p.name.clone())
        });
        *id = name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.get(..8).unwrap_or(id.as_str()).to_owned());
    }
}


/// The tasks actor; absent when the plugin is not loaded or its store failed (67 §7).
fn tasks_actor() -> RpcResult<ActorRef<tasks::Tasks>> {
    lookup::<tasks::Tasks>(tasks::TASKS_ACTOR)
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


/// The document principal behind a call (59 §8.3): a mode-2 guest conn is `Member{pub, device}`,
/// everything else the owner on `device`. Returned with the peer so the caller can mark a guest
/// joined or refused.
fn principal(ext: &Extensions) -> RpcResult<(rpc_core::RpcPeer, document::Principal)> {
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


/// A member call's outcome as the guest's conn sees it (59 §8.3, §11): success scopes the conn
/// (no `doc_join` needed on a return visit), a membership or role refusal counts toward the close.
fn member_outcome<T>(peer: &rpc_core::RpcPeer, r: &RpcResult<T>) {
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
fn join_outcome<T>(guest: &rpc_core::Guest, r: &RpcResult<T>) {
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
pub(crate) fn install_insight_hooks(rt: &Runtime) {
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


/// 158 §7.2: the connection's `x-theuth-client` value for companion-only methods; `None` when
/// the call did not come over the socket (the in-process TUI).
fn client_of(ext: &Extensions) -> Option<&'static str> {
    ext.get::<rpc_core::Client>().map(|c| c.0)
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
        let names: Vec<&str> = into_rpc_module(crate::new_server(rt)).method_names().collect();
        assert!(names.contains(&"resume_turn"), "{names:?}");
    }

    /// 136 §2.7: every `dreaming.*` method is registered under its dotted name.
    #[tokio::test]
    async fn dreaming_methods_are_registered() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let names: Vec<&str> = into_rpc_module(crate::new_server(rt)).method_names().collect();
        for m in dreaming::rpc::METHODS {
            assert!(names.contains(&m), "{m} missing from {names:?}");
        }
    }

    /// 140 §12.5: every `autoresearch.*` method is registered under its dotted name.
    #[tokio::test]
    async fn autoresearch_methods_are_registered() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let names: Vec<&str> = into_rpc_module(crate::new_server(rt)).method_names().collect();
        for m in autoresearch::rpc_impl::METHODS {
            assert!(names.contains(&m), "{m} missing from {names:?}");
        }
    }

    /// 170 D1: `slack_manifest` answers `slack::MANIFEST` verbatim with no Slack
    /// actor running (unconfigured), and the text is the manifest the setup uses.
    #[tokio::test]
    async fn slack_manifest_answers_the_compiled_manifest_without_slack() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
        let req = r#"{"jsonrpc":"2.0","method":"slack_manifest","params":[],"id":1}"#;
        let (resp, _) = methods.raw_json_request(req, 1).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        assert_eq!(v["result"].as_str(), Some(slack::MANIFEST), "{v}");
        let m: serde_yaml::Value = serde_yaml::from_str(slack::MANIFEST).unwrap();
        assert_eq!(m["features"]["slash_commands"][0]["command"], "/theuth");
    }

    /// 188: `config_flag_*` over the wire against a boot config (`running`) and a
    /// different config.yaml in the data dir (`enabled`).
    async fn config_flag_call(
        boot: &str,
        file: Option<&str>,
        method: &str,
        params: &str,
    ) -> (serde_json::Value, tempfile::TempDir) {
        use figment::providers::{Format, Yaml};
        let dir = tempfile::tempdir().unwrap();
        if let Some(f) = file {
            std::fs::write(dir.path().join("config.yaml"), f).unwrap();
        }
        let rt = Runtime::new(
            figment::Figment::from(Yaml::string(boot)),
            dir.path().to_path_buf(),
        );
        let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
        let req = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params},"id":1}}"#);
        let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
        (serde_json::from_str(resp.get()).unwrap(), dir)
    }

    /// 188: `get` reports the file value, the boot value and their difference for each
    /// key; an absent key or section is that section's default (decision and computer
    /// on, a2a off, double `DoubleConfig::default()`, both `allow_private_network` on).
    #[tokio::test]
    async fn config_flag_get_reports_file_boot_and_restart() {
        let flag = |key: &str, enabled: bool, running: bool| serde_json::json!({ "key": key, "enabled": enabled, "running": running, "restart_required": enabled != running });
        let boot = "decision:\n  enabled: false\na2a:\n  enabled: true\n";
        let file = "decision:\n  enabled: true\ncomputer:\n  enabled: false\ndouble:\n  enabled: true\nweb:\n  allow_private_network: false\n";
        for (key, want) in [
            ("decision.enabled", flag("decision.enabled", true, false)),
            ("a2a.enabled", flag("a2a.enabled", false, true)),
            ("computer.enabled", flag("computer.enabled", false, true)),
            ("double.enabled", flag("double.enabled", true, false)),
            (
                "web.allow_private_network",
                flag("web.allow_private_network", false, true),
            ),
            (
                "browser.allow_private_network",
                flag("browser.allow_private_network", true, true),
            ),
        ] {
            let (v, _dir) = config_flag_call(
                boot,
                Some(file),
                "config_flag_get",
                &format!(r#"["{key}"]"#),
            )
            .await;
            assert_eq!(v["result"], want, "{key}: {v}");
        }
        for (key, on) in [
            ("decision.enabled", true),
            ("a2a.enabled", false),
            ("computer.enabled", true),
            ("double.enabled", double::DoubleConfig::default().enabled),
            ("web.allow_private_network", true),
            ("browser.allow_private_network", true),
        ] {
            let (v, _dir) =
                config_flag_call("", None, "config_flag_get", &format!(r#"["{key}"]"#)).await;
            assert_eq!(v["result"], flag(key, on, on), "{key} defaults: {v}");
        }
    }

    /// 188: `set` rewrites only the section's `enabled` line (comments and every other
    /// key byte-for-byte), creates a missing section, and changes nothing live.
    #[tokio::test]
    async fn config_flag_set_writes_only_the_key() {
        let file = "# mine\nagent:\n  model: x  # keep\na2a:\n  listen: 127.0.0.1:1  # port\n  enabled: false # off for now\n";
        let (v, dir) = config_flag_call(
            "",
            Some(file),
            "config_flag_set",
            r#"["a2a.enabled", true]"#,
        )
        .await;
        assert_eq!(
            v["result"],
            serde_json::json!({ "key": "a2a.enabled", "enabled": true, "running": false, "restart_required": true }),
            "{v}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            file.replace("enabled: false #", "enabled: true #")
        );

        let (v, dir) = config_flag_call(
            "",
            Some(file),
            "config_flag_set",
            r#"["computer.enabled", false]"#,
        )
        .await;
        assert_eq!(v["result"]["enabled"], false, "{v}");
        assert_eq!(v["result"]["running"], true, "{v}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            format!("{file}\ncomputer:\n  enabled: false\n")
        );

        let file = "double:\n  retention_days: 30  # short\n  enabled: false\n";
        let (v, dir) = config_flag_call(
            "",
            Some(file),
            "config_flag_set",
            r#"["double.enabled", true]"#,
        )
        .await;
        assert_eq!(
            v["result"],
            serde_json::json!({ "key": "double.enabled", "enabled": true, "running": false, "restart_required": true }),
            "{v}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            file.replace("enabled: false", "enabled: true")
        );

        // 16 §5 (2026-11-11): a non-`enabled` key lands in its own section, beside the rest.
        let file = "web:\n  max_redirects: 3  # mine\n";
        let (v, dir) = config_flag_call(
            "",
            Some(file),
            "config_flag_set",
            r#"["web.allow_private_network", false]"#,
        )
        .await;
        assert_eq!(
            v["result"],
            serde_json::json!({ "key": "web.allow_private_network", "enabled": false, "running": true, "restart_required": true }),
            "{v}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            "web:\n  allow_private_network: false\n  max_redirects: 3  # mine\n"
        );
    }

    /// 188: only the known keys parse; anything else is `INVALID_PARAMS` naming them,
    /// and nothing is written.
    #[tokio::test]
    async fn config_flag_rejects_an_unknown_key() {
        for (method, params) in [
            ("config_flag_get", r#"["dreaming.enabled"]"#),
            ("config_flag_set", r#"["agent.model", true]"#),
        ] {
            let (v, dir) = config_flag_call("", None, method, params).await;
            assert_eq!(v["error"]["code"], -32602, "{v}");
            let text = v["error"].to_string();
            for key in [
                "decision.enabled",
                "a2a.enabled",
                "computer.enabled",
                "double.enabled",
                "web.allow_private_network",
                "browser.allow_private_network",
            ] {
                assert!(text.contains(key), "{key} missing from {text}");
            }
            assert!(!dir.path().join("config.yaml").exists());
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
        let mut methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
        methods.extensions_mut().insert(rpc_core::RpcPeer::internal());
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

    /// 191 (D3): `a2a_forward` is mesh-internal (no `Forwarded` marker: `UNAUTHORIZED`, even
    /// on an internal connection); a forwarder `mesh.messaging_off` lists is refused in words
    /// that name no machine; any other forwarder reaches the owner side (no a2a plugin here:
    /// `OWNER_UNREACHABLE`, which the receiving machine reads as offline).
    #[tokio::test]
    async fn a2a_forward_needs_the_forwarded_marker_and_messaging_on() {
        let call = |forwarder: Option<&str>| {
            let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
            let mut methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
            methods.extensions_mut().insert(rpc_core::RpcPeer::internal());
            if let Some(f) = forwarder {
                methods
                    .extensions_mut()
                    .insert(forward::Forwarded(replica::PeerId(f.into())));
            }
            async move {
                let req = r#"{"jsonrpc":"2.0","method":"a2a_forward","params":["caller-eid","/agents/x/rpc","{}"],"id":1}"#;
                let (resp, _) = methods.raw_json_request(req, 1).await.unwrap();
                let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
                (
                    v["error"]["code"].as_i64(),
                    v["error"]["message"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )
            }
        };
        assert_eq!(call(None).await.0, Some(i64::from(UNAUTHORIZED)));
        mesh::set_messaging_off(vec!["fwd-off-engine".into()]);
        let off = call(Some("fwd-off-engine")).await;
        let on = call(Some("fwd-on-engine")).await;
        mesh::set_messaging_off(Vec::new());
        assert_eq!(
            off,
            (
                Some(i64::from(ASK_FAILED)),
                A2A_FORWARD_MESSAGING_OFF.to_owned()
            )
        );
        assert_eq!(on.0, Some(i64::from(forward::OWNER_UNREACHABLE)), "{on:?}");
    }

    /// 185 R1 A5: the hub side of a forwarded Dream now — the positional
    /// `[RunRequest]` the forwarder sends dispatches into dreaming (no actor
    /// here → `NOT_RUNNING`), never `INVALID_PARAMS`, never a second hop.
    #[tokio::test]
    async fn a_forwarded_dream_now_runs_on_the_hub_with_the_forwarders_params() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
        methods.extensions_mut().insert(rpc_core::RpcPeer::internal());
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

    /// 193 §7: `session_call_tool` refuses a name outside `SESSION_TOOLS` as invalid params
    /// before authorize (even for an unknown session); the six pass the allowlist (an
    /// unauthenticated raw call then fails at authorize, with another error).
    #[tokio::test]
    async fn session_call_tool_allowlist_precedes_authorize() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
        let call = |name: &str| {
            let methods = methods.clone();
            let req = format!(
                r#"{{"jsonrpc":"2.0","method":"session_call_tool","params":["{}","{name}",{{}}],"id":1}}"#,
                uuid::Uuid::now_v7()
            );
            async move {
                let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
                serde_json::from_str::<serde_json::Value>(resp.get()).unwrap()
            }
        };
        let invalid = i64::from(jsonrpsee::types::error::INVALID_PARAMS_CODE);
        let v = call("shell").await;
        assert_eq!(v["error"]["code"].as_i64(), Some(invalid), "{v}");
        for name in SESSION_TOOLS {
            let v = call(name).await;
            assert_ne!(v["error"]["code"].as_i64(), Some(invalid), "{name}: {v}");
            assert!(
                !v["error"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("not offered"),
                "{name}: {v}"
            );
        }
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
        #[cfg(test)]
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


        struct Harness {
            methods: jsonrpsee::Methods,
            tx: broadcast::Sender<Event>,
            focus: FocusMap,
            peer: rpc_core::RpcPeer,
            guard: Option<rpc_core::RpcPeerGuard>,
        }

        impl Harness {
            /// A server whose feed is a ring of `ring` slots, every call authenticated as one peer.
            fn new(ring: usize) -> Self {
                let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
                let mut server = crate::new_server(rt);
                let tx = broadcast::channel(ring).0;
                server.events = tx.clone();
                let focus = server.focus.clone();
                let mut methods: jsonrpsee::Methods = into_rpc_module(server).into();
                let (peer, guard) = rpc_core::RpcPeer::open();
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
        let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
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
        let base: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
        assert_eq!(app_close_code(&base).await, i64::from(UNAUTHORIZED));

        let (peer, _guard) = rpc_core::RpcPeer::open();
        let mut with_peer = base.clone();
        with_peer.extensions_mut().insert(peer);
        // No app actor in this process: past the peer gate, the lookup fails.
        assert_eq!(app_close_code(&with_peer).await, i64::from(NOT_RUNNING));
    }

    /// Dispatch a raw dictation request as a remote (relay) peer on a fresh server.
    async fn remote_dictation_call(method: &str, params: &str) -> serde_json::Value {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
        let (peer, _guard) = rpc_core::RpcPeer::open_remote();
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
        let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
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
        let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt)).into();
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
        #[cfg(test)]
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
                let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt.clone())).into();
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
            let (tx, rx) = broadcast::channel::<rpc_core::Event>(4);
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
            let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt.clone())).into();
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
            let methods: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt.clone())).into();
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

        /// A 1×1 RGBA PNG, standard base64.
        const DOT: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

        /// Bus subscriber handing every `FenceSnapshotRequested` over whole.
        #[derive(kameo::Actor)]
        struct Requests(mpsc::UnboundedSender<FenceSnapshotRequested>);

        impl Message<FenceSnapshotRequested> for Requests {
            type Reply = ();
            async fn handle(&mut self, m: FenceSnapshotRequested, _: &mut Context<Self, ()>) {
                let _ = self.0.send(m);
            }
        }

        /// `rt`'s methods behind `peer`.
        fn methods(rt: &Runtime, peer: rpc_core::RpcPeer) -> jsonrpsee::Methods {
            let mut m: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt.clone())).into();
            m.extensions_mut().insert(peer);
            m
        }

        /// A raw `fence_snapshot` call with positional `params`, in its own task: the whole
        /// JSON-RPC response.
        fn snapshot(
            m: &jsonrpsee::Methods,
            params: &str,
        ) -> tokio::task::JoinHandle<serde_json::Value> {
            let m = m.clone();
            let req = format!(
                r#"{{"jsonrpc":"2.0","method":"fence_snapshot","params":{params},"id":1}}"#
            );
            tokio::spawn(async move {
                let (resp, _) = m.raw_json_request(&req, 1).await.unwrap();
                serde_json::from_str(resp.get()).unwrap()
            })
        }

        fn failed(reason: &str) -> FenceSnapshotAnswer {
            FenceSnapshotAnswer::Failed {
                reason: reason.into(),
                client: Client::Desktop,
            }
        }

        /// 192: a client draws it → `done` with the PNG as standard base64 and its header size.
        #[tokio::test(start_paused = true)]
        async fn the_rpc_returns_the_drawn_png() {
            let (rt, mut events) = harness().await;
            let (peer, _guard) = rpc_core::RpcPeer::open();
            let m = methods(&rt, peer);
            let call = snapshot(&m, r#"["mermaid","graph TD; A-->B",560,"dark",2]"#);
            let (_, id) = events.recv().await.unwrap();
            let started = FenceSnapshotAnswer::Started {
                client: Client::Desktop,
            };
            assert!(answer(&m, id, &started).await);
            let drawn = FenceSnapshotAnswer::Done {
                png: DOT.into(),
                width: 1,
                height: 1,
                client: Client::Desktop,
            };
            assert!(answer(&m, id, &drawn).await);
            let v = call.await.unwrap();
            assert_eq!(
                v["result"],
                serde_json::json!({"outcome":"done","png":DOT,"width":1,"height":1,"error":null}),
                "{v}"
            );
        }

        /// 192: nobody on the bus to draw it → `no_renderer`, every other field null.
        #[tokio::test(start_paused = true)]
        async fn no_subscriber_is_no_renderer() {
            let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
            let (peer, _guard) = rpc_core::RpcPeer::open();
            let v = snapshot(&methods(&rt, peer), r#"["openui","x",560,"light",2]"#)
                .await
                .unwrap();
            assert_eq!(
                v["result"],
                serde_json::json!({"outcome":"no_renderer","png":null,"width":null,"height":null,"error":null}),
                "{v}"
            );
        }

        /// 192: a client that cannot draw it → `failed` with its reason.
        #[tokio::test(start_paused = true)]
        async fn a_failed_client_is_failed_with_its_reason() {
            let (rt, mut events) = harness().await;
            let (peer, _guard) = rpc_core::RpcPeer::open();
            let m = methods(&rt, peer);
            let call = snapshot(&m, r#"["json-render","{",560,"light",2]"#);
            let (_, id) = events.recv().await.unwrap();
            assert!(answer(&m, id, &failed("bad json")).await);
            let v = call.await.unwrap();
            assert_eq!(
                v["result"],
                serde_json::json!({"outcome":"failed","png":null,"width":null,"height":null,"error":"bad json"}),
                "{v}"
            );
        }

        /// 192: `width` is clamped to 240..=1200 and `scale` to 1..=3 before the request goes out.
        #[tokio::test(start_paused = true)]
        async fn width_and_scale_are_clamped() {
            let (rt, _events) = harness().await;
            let (tx, mut seen) = mpsc::unbounded_channel();
            let req = Requests::spawn(Requests(tx));
            rt.subscribe::<Requests, FenceSnapshotRequested>(&req).await;
            let (peer, _guard) = rpc_core::RpcPeer::open();
            let m = methods(&rt, peer);
            for (params, want) in [
                (r#"["mermaid","a",10,"dark",9]"#, (240, 3)),
                (r#"["mermaid","b",5000,"dark",0]"#, (1200, 1)),
            ] {
                let call = snapshot(&m, params);
                let r = seen.recv().await.unwrap();
                assert_eq!((r.width, r.scale), want, "{params}");
                assert!(answer(&m, r.id, &failed("x")).await);
                assert_eq!(call.await.unwrap()["result"]["outcome"], "failed");
            }
        }

        /// 192: a mode-2 document guest (and an unauthenticated call) is refused before any
        /// request goes out.
        #[tokio::test(start_paused = true)]
        async fn guests_are_refused() {
            let (rt, mut events) = harness().await;
            let (guest, _guard) = rpc_core::RpcPeer::open_guest([7; 32], [9; 32]);
            let params = r#"["mermaid","a",560,"dark",2]"#;
            let v = snapshot(&methods(&rt, guest), params).await.unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(i64::from(UNAUTHORIZED)),
                "{v}"
            );
            let bare: jsonrpsee::Methods = into_rpc_module(crate::new_server(rt.clone())).into();
            let v = snapshot(&bare, params).await.unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(i64::from(UNAUTHORIZED)),
                "{v}"
            );
            assert!(events.try_recv().is_err(), "no request published");
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
        let (peer, _guard) = rpc_core::RpcPeer::open_guest([7; 32], [9; 32]);
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
