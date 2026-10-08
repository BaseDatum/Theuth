//! One external harness bound to one ACP session (design doc 06 §3.2, §3.7,
//! §8.1–§8.3; 32 §6.5). [`Harness::connect`] opens the [`Transport`] — a
//! subprocess, or a [`Dialer`]'s line duplex (a node over a link conn) — runs
//! `initialize` → auth → `session/load` | `session/new`, and hands back a
//! handle; `session/update` notifications and `session/request_permission`
//! requests arrive on the caller's channel as [`Update`]s in wire order.
//!
//! The SDK connection lives inside one `tokio::spawn`ed future; the child
//! process (its whole process group) or the dialled duplex dies when that
//! future ends, which happens on [`Harness::close`], when the last handle is
//! dropped, or when the transport's incoming side hits EOF.

use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentRequest, AuthCapabilities, AuthMethod, AuthenticateRequest, CancelNotification,
    ClientCapabilities, ContentBlock, Error, ErrorCode, Implementation, InitializeRequest,
    InitializeResponse, LoadSessionRequest, McpServer, Meta, NewSessionRequest, PermissionOption,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue, SessionConfigSelectOptions, SessionConfigValueId, SessionId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, StopReason, ToolCallUpdate,
    Usage,
};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo, JsonRpcRequest, LineDirection, Lines,
    Responder, UntypedMessage, is_incoming_transport_closed,
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::PeerMessage;

/// [`probe`] gives up on `initialize` + `session/new` after this long.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the caller gets from the connection: `session/update` notifications,
/// `session/request_permission` requests, subagent announces (141 §7.1) and
/// `_theuth/peer_message` extension notifications (67 §8), in wire order.
pub enum Update {
    /// A `session/update`. `session` is the wire `params.sessionId`: the
    /// harness's own id for the driven session ([`Harness::session_id`]), or a
    /// child's external id for RFD child traffic / an opencode
    /// `child_update{type:"update"}` (141 §6.1, §6.2).
    Session {
        /// Wire `params.sessionId`.
        session: String,
        /// The notification's payload.
        update: SessionUpdate,
    },
    /// A subagent announce/upsert from any source (141 §6): identity, state
    /// and detail of a watch-only child the harness runs inside itself.
    Subagent(ChildAnnounce),
    /// A `session/request_permission` request; answer it exactly once.
    /// [`Permission::session`] is the asking session (a child's id for RFD
    /// child asks).
    Permission(Permission),
    /// A remote theuth child's `send_message{to: "parent"}` ([`crate::PEER_MESSAGE_METHOD`]).
    Peer(PeerMessage),
    /// stdout EOF / process exit: the exit error text (with the stderr tail on
    /// a non-zero status) or `process exited`.
    Closed(String),
}

impl fmt::Debug for Update {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Update::Session { session, update } => f
                .debug_struct("Session")
                .field("session", session)
                .field("update", update)
                .finish(),
            Update::Subagent(a) => f.debug_tuple("Subagent").field(a).finish(),
            Update::Permission(p) => f
                .debug_struct("Permission")
                .field("session", &p.session)
                .field("tool_call", &p.tool_call)
                .finish(),
            Update::Peer(m) => f.debug_tuple("Peer").field(&m.id).finish(),
            Update::Closed(why) => f.debug_tuple("Closed").field(why).finish(),
        }
    }
}

/// Which feed produced a [`ChildAnnounce`] (141 §3 "Source"). The agent
/// crate stores it as `ObservedSource` with the same `snake_case` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildSource {
    /// ACP RFD `subagent_update` + child-addressed `session/update`s (any compliant agent).
    Rfd,
    /// opencode 2.x `opencode/session/child_update`.
    OpencodeExt,
    /// opencode 1.x `task` tool-call lifecycle only (no child traffic).
    OpencodeTask,
    /// omp `task` progress rows + on-disk transcript tail.
    OmpTask,
}

/// How a watch-only child's cost relates to its parent's (141 §7.6, D8).
/// Chosen by the adapter that sees the frame; stored by the agent crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum CostRule {
    /// The child's cost is its own meter rows and rolls up (omp background
    /// spawns: excluded from the harness's `usage_update`).
    Counted,
    /// Already inside the parent's meter; display-only (omp sync spawns).
    InsideParent,
    /// Reported by the harness, not aggregated (RFD `idle.usage`).
    DisplayOnly,
    /// Not on the wire ("—"): opencode 1.x / 2.x.
    Unreported,
}

impl CostRule {
    /// The stored string form (`sessions.observed_cost_rule`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Counted => "counted",
            Self::InsideParent => "inside_parent",
            Self::DisplayOnly => "display_only",
            Self::Unreported => "unreported",
        }
    }
}

impl fmt::Display for CostRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CostRule {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "counted" => Ok(Self::Counted),
            "inside_parent" => Ok(Self::InsideParent),
            "display_only" => Ok(Self::DisplayOnly),
            "unreported" => Ok(Self::Unreported),
            _ => Err(format!("unknown CostRule: {s:?}")),
        }
    }
}

/// Lifecycle of a watch-only child as one source reports it (141 §5.2).
/// `Idle` carries the RFD stop reason and display-only usage; `Failed`
/// carries the harness's error text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    /// Announced, not started (omp `pending`, opencode `created`).
    Pending,
    /// The harness is working on it.
    Running,
    /// Waits on a human (permission/elicitation).
    RequiresAction,
    /// Turn ended; the harness may prompt it again (RFD `idle`).
    Idle {
        /// RFD `stopReason`; `"cancelled"` maps to the cancelled state downstream.
        stop_reason: Option<String>,
        /// RFD `idle.usage`, display-only (141 D8).
        usage: Option<Usage>,
    },
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed {
        /// Error text (opencode `error.message`, omp `retryFailure.errorMessage`).
        error: Option<String>,
    },
    /// Stopped by a cancel (opencode `interrupted`, omp `aborted`).
    Cancelled,
    /// Lost contact / activity unobservable (RFD `unknown`).
    Unknown,
}

/// A subagent announce/upsert normalized from any source (141 §3 "Announce",
/// §7.1). Identity is `(parent, source, child)`; every `Option` field is
/// `None` = unchanged, so a status-only frame touches only `state`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChildAnnounce {
    /// Which feed produced this announce.
    pub source: ChildSource,
    /// The harness's own id for the child (RFD `subagentUpdate.sessionId`,
    /// opencode `childSessionId`, omp `progress[].id`, opencode 1.x
    /// `task:<toolCallId>` until the real id is known).
    pub child: String,
    /// External id of the parent; `None` = the driven session (depth 1).
    /// omp nested `a.b` → `Some("a")`; opencode `parentSessionId != root`.
    pub parent: Option<String>,
    /// Display name / title; `None` = unchanged.
    pub title: Option<String>,
    /// Roster card second line (RFD `description`, opencode prompt head, omp `assignment` head).
    pub description: Option<String>,
    /// The child's own model when the source reports one (omp `resolvedModel`,
    /// opencode 1.x `metadata.model`).
    pub model: Option<String>,
    /// Lifecycle; `None` = unchanged.
    pub state: Option<ChildState>,
    /// The harness accepts `session/cancel` for this child (RFD `capabilities.cancel`); `None` = unchanged.
    pub can_cancel: Option<bool>,
    /// The spawning tool call id in the parent's transcript, when known.
    pub tool_call: Option<String>,
    /// Cost rule (141 §7.6); `None` = unchanged (the store defaults per source).
    pub cost_rule: Option<CostRule>,
    /// Source-specific live fields, merged shallowly into `observed_detail`
    /// (141 §6 mapping tables). Empty = unchanged.
    #[serde(default)]
    pub detail: serde_json::Map<String, serde_json::Value>,
}

impl ChildAnnounce {
    /// Identity only; every other field unchanged.
    #[must_use]
    pub fn new(source: ChildSource, child: impl Into<String>) -> Self {
        Self {
            source,
            child: child.into(),
            parent: None,
            title: None,
            description: None,
            model: None,
            state: None,
            can_cancel: None,
            tool_call: None,
            cost_rule: None,
            detail: serde_json::Map::new(),
        }
    }
}

/// A permission request; answer exactly once via [`Permission::respond`].
pub struct Permission {
    /// Wire `params.sessionId`: the driven session, or an RFD child's
    /// external id when the child itself asks (141 §6.1).
    pub session: String,
    /// The tool call asking (title/kind/raw_input live under `.fields`).
    pub tool_call: ToolCallUpdate,
    /// Options offered by the harness (`allow_once`, `reject_once`, …).
    pub options: Vec<PermissionOption>,
    responder: Responder<RequestPermissionResponse>,
}

impl Permission {
    /// Send the outcome back to the harness.
    pub fn respond(self, outcome: RequestPermissionOutcome) {
        if let Err(e) = self
            .responder
            .respond(RequestPermissionResponse::new(outcome))
        {
            tracing::warn!("acp permission response: {e}");
        }
    }
}

/// Outgoing half of a [`LineDuplex`]: one JSON-RPC message per `String`, no newline.
pub type LineSink = Pin<Box<dyn futures::Sink<String, Error = std::io::Error> + Send>>;
/// Incoming half of a [`LineDuplex`]: one JSON-RPC message per `String`, no newline.
pub type LineStream = Pin<Box<dyn futures::Stream<Item = std::io::Result<String>> + Send>>;

/// A dialled connection to a harness (32 §4.3): one JSON-RPC message per
/// `String`, no line framing — the record is the frame.
pub struct LineDuplex {
    /// Client → agent.
    pub outgoing: LineSink,
    /// Agent → client; the stream ending is the connection closing.
    pub incoming: LineStream,
}

/// Opens a fresh [`LineDuplex`] per connection (32 §6.5: one link conn per
/// remote child). `Err` is the text the model and the user see.
#[async_trait::async_trait]
pub trait Dialer: Send + Sync + fmt::Debug {
    /// Open a new connection.
    async fn dial(&self) -> Result<LineDuplex, String>;
    /// How the dialled side names this caller (a node's view of the parent:
    /// its device id, 32 §4.1); `None` when the transport has no such notion.
    fn caller_id(&self) -> Option<String> {
        None
    }
    /// The dialled engine's mesh id (67 §4.1), so `agent: "<id prefix>"`
    /// resolves too; `None` for a transport without one (a 32 node).
    fn engine_id(&self) -> Option<&str> {
        None
    }
}

/// How the harness is reached (32 §6.5).
#[derive(Debug)]
pub enum Transport {
    /// Spawn a process ([`crate::Entry::config`]) and speak ACP on its stdio.
    Process(AcpAgentConfig),
    /// Dial a running harness (a node) and speak ACP over the returned duplex.
    Dial(Arc<dyn Dialer>),
}

impl Transport {
    /// The process configuration, for the `login` command line; `None` for a dial.
    fn config(&self) -> Option<&AcpAgentConfig> {
        match self {
            Self::Process(c) => Some(c),
            Self::Dial(_) => None,
        }
    }
}

/// How to connect.
pub struct Connect {
    /// Process to spawn or harness to dial.
    pub transport: Transport,
    /// Working directory for `session/new` / `session/load` (a process itself
    /// inherits theuth's cwd, 06 §8.4; for a node it is a path on the node).
    pub cwd: PathBuf,
    /// Existing ACP session id to `session/load` when the agent advertises
    /// `loadSession`; else a new session.
    pub resume: Option<String>,
    /// Where a child process's stderr lines are appended (created and left
    /// empty for a dial transport).
    pub stderr_log: PathBuf,
    /// theuth-to-theuth instructions for `session/new` `_meta` (67 §8);
    /// `None` for a harness that is not theuth. Not sent on `session/load`.
    pub meta: Option<crate::SessionMeta>,
    /// Switch the harness to this model after `session/new` | `session/load`
    /// through its `category: "model"` config option (67 §8; omp and
    /// opencode announce one; opencode reads no `_meta`, 06 §8.4). A
    /// harness without the option keeps its own and the pick is logged.
    pub model: Option<String>,
    /// Switch the harness to this effort after the model through its
    /// `category: "thought_level"` option (75 §8, 06 §8.4): applied when the
    /// option exists and lists the value; otherwise the harness keeps its
    /// default and the miss is logged once (opencode announces `effort` only
    /// for models with variants, so a miss is ordinary).
    pub effort: Option<String>,
    /// MCP servers for `session/new` / `session/load` `mcpServers` (75 §9):
    /// the theuth shim, or nothing.
    pub mcp_servers: Vec<McpServer>,
}

/// One selectable value of a [`ConfigOption`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConfigValue {
    /// What `set_config_option` sends.
    pub value: String,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
}

/// A harness's session config option (75 §8): theuth's flat, wire-ready view
/// of the SDK's `SessionConfigOption`. `category` is the SDK's serde name
/// (`model`, `thought_level`, `mode`, …); grouped selects are flattened; a
/// boolean option lists `true`/`false`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConfigOption {
    /// The `configId` to set it with.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `model`, `thought_level`, `mode`, `model_config`, or the harness's own word.
    pub category: Option<String>,
    /// Current value (`options[].value`).
    pub current: String,
    /// Selectable values, in the harness's order.
    pub options: Vec<ConfigValue>,
}

/// Reply of `set_harness_option` (75 §8): the option list after the change,
/// or the list as it stands when the change is queued behind the turn in flight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SetOptionResult {
    /// The harness's options now.
    pub options: Vec<ConfigOption>,
    /// The change is queued and lands when the current turn ends.
    pub pending: bool,
}

impl From<&SessionConfigOption> for ConfigOption {
    fn from(o: &SessionConfigOption) -> Self {
        let (current, options) = match &o.kind {
            SessionConfigKind::Select(s) => {
                let flat: Vec<&agent_client_protocol::schema::v1::SessionConfigSelectOption> =
                    match &s.options {
                        SessionConfigSelectOptions::Ungrouped(v) => v.iter().collect(),
                        SessionConfigSelectOptions::Grouped(g) => {
                            g.iter().flat_map(|g| g.options.iter()).collect()
                        }
                        _ => Vec::new(),
                    };
                (
                    s.current_value.0.to_string(),
                    flat.into_iter()
                        .map(|v| ConfigValue {
                            value: v.value.0.to_string(),
                            name: v.name.clone(),
                            description: v.description.clone(),
                        })
                        .collect(),
                )
            }
            SessionConfigKind::Boolean(b) => (
                b.current_value.to_string(),
                ["true", "false"]
                    .into_iter()
                    .map(|v| ConfigValue {
                        value: v.into(),
                        name: v.into(),
                        description: None,
                    })
                    .collect(),
            ),
            _ => (String::new(), Vec::new()),
        };
        Self {
            id: o.id.0.to_string(),
            name: o.name.clone(),
            category: o.category.as_ref().and_then(|c| {
                serde_json::to_value(c)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
            }),
            current,
            options,
        }
    }
}

/// One in-flight `session/prompt`: resolves with the stop reason (serde name,
/// `end_turn` …) and the response's `usage` when the harness sent one
/// (`unstable_end_turn_token_usage`, 75 §7 — raw as sent: omp and opencode ≥ 2
/// a per-turn sum, opencode 1.x the last request's tokens; see
/// [`Harness::usage_is_lower_bound`]), or `Err(text)` on transport failure.
/// `'static`, so it can be awaited from a spawned task while the [`Harness`]
/// stays with its owner.
pub type Turn = BoxFuture<'static, Result<(String, Option<Usage>), String>>;

/// The session's `configOptions`, shared by every [`Harness`] clone and the
/// connection's notification handler.
type Options = Arc<Mutex<Vec<SessionConfigOption>>>;

/// A live connection to one harness process with one ACP session. Cheap to
/// clone; every clone shares the connection, and [`Harness::close`] on any of
/// them (or dropping the last one) ends it and kills the process group.
#[derive(Clone)]
pub struct Harness {
    cx: ConnectionTo<Agent>,
    session: SessionId,
    resumed: bool,
    /// `initialize.agentInfo` (name/version), when the harness sent one.
    agent: Option<Implementation>,
    /// The harness's `configOptions` (75 §8): from `session/new` | `session/load`,
    /// every `config_option_update`, every `set_config_option` response.
    options: Options,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl fmt::Debug for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Harness")
            .field("session", &self.session)
            .field("resumed", &self.resumed)
            .finish_non_exhaustive()
    }
}

/// Result of [`probe`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    /// `initialize` and `session/new` succeeded.
    pub ok: bool,
    /// `ok` | `login required: <cmd>` | `spawn failed: <why>` (the
    /// [`crate::EntryInfo::status`] strings).
    pub status: String,
}

/// The spawned connection before a session exists: the handle, the shutdown
/// trigger, and the connection task (its result is the process exit text).
/// Dropping it ends the connection.
struct Link {
    cx: ConnectionTo<Agent>,
    shutdown: oneshot::Sender<()>,
    done: Option<JoinHandle<Result<(), Error>>>,
    /// What a clean end of the connection task is called: `process exited`
    /// for a subprocess, `connection closed` for a dial.
    closed: &'static str,
    /// See [`Harness::options`]; the notification handler writes it.
    options: Options,
}

/// Why the handshake could not open a session.
enum Fail {
    /// Authentication required and not doable from here: `cmd` is what to
    /// run in a terminal (terminal-type method) or the method's name.
    Login(String),
    /// Anything else, already rendered.
    Other(String),
}

/// What [`Link::open`] established.
struct Opened {
    session: SessionId,
    /// `session/load` succeeded (resume honoured).
    resumed: bool,
    options: Vec<SessionConfigOption>,
    /// `initialize.agentInfo`.
    agent: Option<Implementation>,
}

impl Link {
    /// Open the transport and wait for the connection handle.
    async fn spawn(
        transport: Transport,
        stderr_log: &Path,
        tx: UnboundedSender<Update>,
    ) -> Result<Link, String> {
        let (cx_tx, cx_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (shutdown, shutdown_rx) = oneshot::channel::<()>();
        let notify_tx = tx.clone();
        let peer_tx = tx.clone();
        let permission_tx = tx;
        let shared: Options = Arc::default();
        let options = shared.clone();
        let builder = Client
            .builder()
            .on_receive_notification(
                async move |n: SessionNotification, _cx| {
                    // 75 §8: the option list is a session fact; keep it
                    // current before the actor sees the update.
                    if let SessionUpdate::ConfigOptionUpdate(u) = &n.update
                        && let Ok(mut o) = options.lock()
                    {
                        o.clone_from(&u.config_options);
                    }
                    let _ = notify_tx.send(Update::Session {
                        session: n.session_id.0.to_string(),
                        update: n.update,
                    });
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            // Every other notification: the peer-message extension is ours
            // (67 §8), opencode's sub-agent updates ride as session updates
            // (06 §8.4); anything else is dropped, as the SDK would drop it.
            // 141 C stub: W1 replaces the `child_update` branch with
            // `opencode::child_update(&n) -> Vec<Update>` (announce + child
            // traffic keyed by `childSessionId`, 141 §6.2). Until then the
            // flattened update is attributed to the root session as before.
            .on_receive_notification(
                async move |n: UntypedMessage, _cx| {
                    if let Some(m) = PeerMessage::parse(&n) {
                        let _ = peer_tx.send(Update::Peer(m));
                    } else if let Some(u) = child_update(&n) {
                        let session = n
                            .params
                            .get("rootSessionId")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let _ = peer_tx.send(Update::Session { session, update: u });
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |r: RequestPermissionRequest, responder, _cx| {
                    let _ = permission_tx.send(Update::Permission(Permission {
                        session: r.session_id.0.to_string(),
                        tool_call: r.tool_call,
                        options: r.options,
                        responder,
                    }));
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            // Mandatory catch-all (06 §8.3): the SDK would otherwise queue any
            // unhandled session-scoped request (`fs/*`, `terminal/*`) forever.
            .on_receive_request(
                async |_r: AgentRequest, responder, _cx| {
                    let _ = responder.respond_with_error(Error::method_not_found());
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            );
        let main = |cx: ConnectionTo<Agent>| async move {
            let _ = cx_tx.send(cx.clone());
            // The connection (and the child) lives until `close`, the last
            // handle drop, or the transport's incoming EOF.
            tokio::select! {
                _ = shutdown_rx => {}
                () = cx.incoming_closed() => {}
            }
            Ok(())
        };
        let log = Mutex::new(open_log(stderr_log));
        let (connection, closed): (BoxFuture<'static, Result<(), Error>>, &'static str) =
            match transport {
                Transport::Process(config) => {
                    let agent = AcpAgent::new(config).with_debug(move |line, dir| {
                        if dir == LineDirection::Stderr
                            && let Ok(mut guard) = log.lock()
                            && let Some(f) = guard.as_mut()
                        {
                            let _ = writeln!(f, "{line}");
                        }
                    });
                    (
                        Box::pin(builder.connect_with(agent, main)),
                        "process exited",
                    )
                }
                Transport::Dial(dialer) => {
                    let duplex = dialer.dial().await?;
                    let lines = Lines::new(duplex.outgoing, duplex.incoming);
                    (
                        Box::pin(builder.connect_with(lines, main)),
                        "connection closed",
                    )
                }
            };
        let done = tokio::spawn(connection);
        match cx_rx.await {
            Ok(cx) => Ok(Link {
                cx,
                shutdown,
                done: Some(done),
                closed,
                options: shared,
            }),
            Err(_) => Err(format!("spawn failed: {}", exit_text(done.await, closed))),
        }
    }

    async fn request<R: JsonRpcRequest>(&self, req: R) -> Result<R::Response, Error> {
        self.cx.send_request(req).block_task().await
    }

    /// Text for a failed request: on transport EOF wait for the exit status,
    /// whose text carries the stderr tail.
    async fn explain(&mut self, e: Error) -> String {
        if is_incoming_transport_closed(&e)
            && let Some(done) = self.done.take()
        {
            return exit_text(done.await, self.closed);
        }
        error_text(&e)
    }

    /// `initialize`, then `session/load` (when `resume` is set and the agent
    /// advertises `loadSession`; a failing load falls back to a new session)
    /// or `session/new`. `auth_required` → `authenticate` once with an
    /// agent-type method when `authenticate`, else [`Fail::Login`]. Returns
    /// the session, whether it was loaded, its `configOptions` and the
    /// agent's `agentInfo`.
    async fn open(
        &mut self,
        config: Option<&AcpAgentConfig>,
        cwd: &Path,
        resume: Option<String>,
        meta: Option<Meta>,
        mcp: &[McpServer],
        authenticate: bool,
    ) -> Result<Opened, Fail> {
        let init: InitializeResponse = match self.request(initialize_request()).await {
            Ok(i) => i,
            Err(e) => return Err(Fail::Other(self.explain(e).await)),
        };
        let mut load = resume.filter(|_| init.agent_capabilities.load_session);
        let mut authenticated = false;
        loop {
            let opened = match &load {
                Some(sid) => self
                    .request(
                        LoadSessionRequest::new(SessionId::new(sid.as_str()), cwd)
                            .mcp_servers(mcp.to_vec()),
                    )
                    .await
                    .map(|r| {
                        (
                            SessionId::new(sid.as_str()),
                            true,
                            r.config_options.unwrap_or_default(),
                        )
                    }),
                None => self
                    .request(
                        NewSessionRequest::new(cwd)
                            .meta(meta.clone())
                            .mcp_servers(mcp.to_vec()),
                    )
                    .await
                    .map(|r| (r.session_id, false, r.config_options.unwrap_or_default())),
            };
            match opened {
                Ok((session, resumed, options)) => {
                    return Ok(Opened {
                        session,
                        resumed,
                        options,
                        agent: init.agent_info,
                    });
                }
                Err(e) if e.code == ErrorCode::AuthRequired && !authenticated => {
                    let agent_method = init.auth_methods.iter().find_map(|m| match m {
                        AuthMethod::Agent(a) => Some(a),
                        _ => None,
                    });
                    match agent_method {
                        Some(m) if authenticate => {
                            if let Err(e) =
                                self.request(AuthenticateRequest::new(m.id.clone())).await
                            {
                                let why = self.explain(e).await;
                                return Err(Fail::Other(format!(
                                    "authenticate ({}): {why}",
                                    m.name
                                )));
                            }
                            authenticated = true;
                        }
                        Some(m) => return Err(Fail::Login(m.name.clone())),
                        None => return Err(Fail::Login(login_command(config, &init.auth_methods))),
                    }
                }
                Err(e) if load.is_some() && !is_incoming_transport_closed(&e) => {
                    tracing::warn!("acp session/load failed, starting a new session: {e}");
                    load = None;
                }
                Err(e) => return Err(Fail::Other(self.explain(e).await)),
            }
        }
    }
}

impl Harness {
    /// Open the transport, `initialize` (client capabilities: no `fs`, no
    /// `terminal`, `auth.terminal: true`; client info `theuth`/version), authenticate
    /// per 06 §3.7 (agent-type method → `authenticate` once and retry; only a
    /// terminal-type method → `Err("not logged in — run `<cmd>` in a terminal, then retry")`),
    /// then `session/load` (when `resume` is set and `loadSession` is advertised)
    /// or `session/new {cwd}`. Updates flow to `tx`. The `Err` text is what the
    /// model and the user see; on process exit it includes the last stderr lines.
    pub async fn connect(c: Connect, tx: UnboundedSender<Update>) -> Result<Harness, String> {
        let Connect {
            transport,
            cwd,
            resume,
            stderr_log,
            meta,
            model: want,
            effort,
            mcp_servers,
        } = c;
        let config = transport.config().cloned();
        let meta = meta.as_ref().and_then(crate::SessionMeta::to_meta);
        let mut link = Link::spawn(transport, &stderr_log, tx.clone()).await?;
        let Opened {
            session,
            resumed,
            mut options,
            agent,
        } = match link
            .open(config.as_ref(), &cwd, resume, meta, &mcp_servers, true)
            .await
        {
            Ok(s) => s,
            Err(Fail::Login(cmd)) => {
                return Err(format!(
                    "not logged in — run `{cmd}` in a terminal, then retry"
                ));
            }
            Err(Fail::Other(why)) => return Err(why),
        };
        // 06 §8.4: the version decides the usage semantics; say it once per child.
        match &agent {
            Some(a) => tracing::info!(session = %session.0, "acp harness {} {}", a.name, a.version),
            None => tracing::info!(session = %session.0, "acp harness (no agentInfo)"),
        }
        // 06 §8.4: `session/new` `_meta` sets nothing on opencode; the model
        // and effort go through `set_config_option`, after `session/load` too.
        if let Some(want) = want
            && current_model(&options).as_deref() != Some(want.as_str())
        {
            match set_option(&link.cx, &session, &options, "model", &want).await {
                Ok(o) => options = o,
                Err(e) => tracing::warn!("acp model `{want}` not applied: {e}"),
            }
        }
        // The effort option may only exist once the model is set (opencode
        // announces it for models with variants); a miss keeps the default.
        if let Some(effort) = effort
            && current_value(&options, "effort").as_deref() != Some(effort.as_str())
        {
            match set_option(&link.cx, &session, &options, "effort", &effort).await {
                Ok(o) => options = o,
                Err(e) => tracing::warn!("acp effort `{effort}` not applied: {e}"),
            }
        }
        let Link {
            cx,
            shutdown,
            done,
            closed,
            options: shared,
        } = link;
        *shared.lock().unwrap_or_else(|p| p.into_inner()) = options;
        if let Some(done) = done {
            tokio::spawn(async move {
                let _ = tx.send(Update::Closed(exit_text(done.await, closed)));
            });
        }
        Ok(Harness {
            cx,
            session,
            resumed,
            agent,
            options: shared,
            shutdown: Arc::new(Mutex::new(Some(shutdown))),
        })
    }

    /// `initialize.agentInfo` as `(name, version)`, when sent.
    #[must_use]
    pub fn agent_info(&self) -> Option<(&str, &str)> {
        self.agent
            .as_ref()
            .map(|a| (a.name.as_str(), a.version.as_str()))
    }

    /// 06 §8.4: `PromptResponse.usage` from this harness is the last
    /// request's tokens, not the turn's sum — opencode before 2.0 (upstream
    /// #1860). Everything else (omp, opencode ≥ 2, unknown) is taken per turn.
    #[must_use]
    pub fn usage_is_lower_bound(&self) -> bool {
        usage_is_lower_bound(self.agent.as_ref())
    }

    /// The model the harness runs on now ([`current_model`] of its live
    /// option list); `None` when it reports none.
    #[must_use]
    pub fn model(&self) -> Option<String> {
        current_model(&self.options.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// The harness's session config options as they stand (75 §8).
    #[must_use]
    pub fn options(&self) -> Vec<ConfigOption> {
        self.options
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(ConfigOption::from)
            .collect()
    }

    /// Would [`Self::set_config_option`]`(key, value)` pass the local checks
    /// now (the option exists, the value is listed)? The refusal text otherwise.
    pub fn check_option(&self, key: &str, value: &str) -> Result<(), String> {
        resolve(
            &self.options.lock().unwrap_or_else(|p| p.into_inner()),
            key,
            value,
        )
        .map(drop)
    }

    /// `session/set_config_option` (75 §8): `key` is an option id or the
    /// alias `model` / `effort` (see [`resolve`]). A missing option or an
    /// unlisted value is refused with the list before anything reaches the
    /// harness; a harness error carries the option's values. The response
    /// refreshes [`Self::options`], which is returned.
    pub async fn set_config_option(
        &self,
        key: &str,
        value: &str,
    ) -> Result<Vec<ConfigOption>, String> {
        let current = self
            .options
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let fresh = set_option(&self.cx, &self.session, &current, key, value).await?;
        *self.options.lock().unwrap_or_else(|p| p.into_inner()) = fresh;
        Ok(self.options())
    }

    /// The harness's session id.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session.0
    }

    /// Whether `session/load` was used (resume honoured).
    #[must_use]
    pub fn resumed(&self) -> bool {
        self.resumed
    }

    /// One `session/prompt`; the returned future resolves with the stop reason
    /// string (`end_turn` …) and the response's token usage, or `Err(text)` on
    /// transport failure. Await it from a task, never from inside an SDK
    /// handler. Dropping it cancels the request.
    pub fn prompt(&self, text: String) -> Turn {
        let cx = self.cx.clone();
        let session = self.session.clone();
        Box::pin(async move {
            let r = cx
                .send_request(PromptRequest::new(session, vec![ContentBlock::from(text)]))
                .block_task()
                .await
                .map_err(|e| error_text(&e))?;
            Ok((stop_reason(r.stop_reason), r.usage))
        })
    }

    /// `session/cancel` notification; the in-flight prompt then resolves `cancelled`.
    pub fn cancel(&self) -> Result<(), String> {
        self.cx
            .send_notification(CancelNotification::new(self.session.clone()))
            .map_err(|e| error_text(&e))
    }

    /// `session/cancel` for a child session the harness announced with
    /// `capabilities.cancel` (141 §7.1, RFD); `child` is its external id.
    /// The harness decides what cancelling means for it; theuth only asks.
    pub fn cancel_session(&self, child: &str) -> Result<(), String> {
        self.cx
            .send_notification(CancelNotification::new(SessionId::new(child)))
            .map_err(|e| error_text(&e))
    }

    /// End the connection; kills the process group.
    pub fn close(self) {
        if let Ok(mut guard) = self.shutdown.lock() {
            guard.take();
        }
    }
}

/// The active model in a harness's session `configOptions` (06 §8.2): the
/// current value of the `model`-category select, else of the first select
/// whose id or name mentions "model" (a harness without categories). The raw
/// value (`anthropic/claude-opus-4-8`), not its display name. Both omp and
/// opencode report `{"id":"model","category":"model","type":"select"}`.
#[must_use]
pub fn current_model(options: &[SessionConfigOption]) -> Option<String> {
    model_select(options).and_then(select_value)
}

/// The current value of the select option `key` names (see [`find_option`]);
/// `None` when the option is missing or not a select.
fn current_value(options: &[SessionConfigOption], key: &str) -> Option<String> {
    find_option(options, key).ok().and_then(select_value)
}

fn select_value(o: &SessionConfigOption) -> Option<String> {
    match &o.kind {
        SessionConfigKind::Select(s) => Some(s.current_value.0.to_string()),
        _ => None,
    }
}

/// See [`Harness::usage_is_lower_bound`]: `agentInfo.name` contains
/// `opencode` and the version's major part parses below 2.
#[must_use]
pub fn usage_is_lower_bound(agent: Option<&Implementation>) -> bool {
    agent.is_some_and(|a| {
        a.name.to_ascii_lowercase().contains("opencode")
            && a.version
                .trim_start_matches(|c: char| !c.is_ascii_digit())
                .split('.')
                .next()
                .and_then(|major| major.parse::<u32>().ok())
                .is_some_and(|major| major < 2)
    })
}

/// opencode ≥ 2's extension notification carrying a sub-agent's updates
/// (06 §8.4), sent only to a client that advertised
/// `_meta["opencode/child-session-updates"]`.
pub const CHILD_UPDATE_METHOD: &str = "opencode/session/child_update";

/// The `session/update` payload inside an [`CHILD_UPDATE_METHOD`]
/// notification (`{rootSessionId, childSessionId, …, type: "update", update}`),
/// as this session's own update: the sub-agent's tool calls keep their
/// `<child>:<id>` ids and `_meta["opencode/child-session"]`, its
/// `agent_message_chunk`s ride as thought (activity to show, not this turn's
/// result text). `None` for any other method, a `type: "status"` event, or
/// an update kind that is not tool/thought traffic.
#[must_use]
pub fn child_update(n: &UntypedMessage) -> Option<SessionUpdate> {
    if n.method != CHILD_UPDATE_METHOD || n.params.get("type")?.as_str()? != "update" {
        return None;
    }
    let mut update = n.params.get("update")?.clone();
    if update
        .get("sessionUpdate")
        .and_then(serde_json::Value::as_str)
        == Some("agent_message_chunk")
    {
        update["sessionUpdate"] = "agent_thought_chunk".into();
    }
    match serde_json::from_value::<SessionUpdate>(update).ok()? {
        u @ (SessionUpdate::ToolCall(_)
        | SessionUpdate::ToolCallUpdate(_)
        | SessionUpdate::AgentThoughtChunk(_)) => Some(u),
        _ => None,
    }
}

/// The model select itself (see [`current_model`]).
fn model_select(options: &[SessionConfigOption]) -> Option<&SessionConfigOption> {
    options
        .iter()
        .find(|o| o.category == Some(SessionConfigOptionCategory::Model))
        .or_else(|| {
            options.iter().find(|o| {
                o.id.0.to_ascii_lowercase().contains("model")
                    || o.name.to_ascii_lowercase().contains("model")
            })
        })
}

/// 75 §8: the option `key` names in `options`, by exact id, else by alias —
/// `model` → the model select ([`model_select`]), `effort` | `thought_level` |
/// `thinking` → the `thought_level`-category option (omp `thinking`,
/// opencode `effort`). `Err` names the ids on offer.
fn find_option<'a>(
    options: &'a [SessionConfigOption],
    key: &str,
) -> Result<&'a SessionConfigOption, String> {
    let by_id = options.iter().find(|o| &*o.id.0 == key);
    let (label, found) = match key {
        "model" => ("model", by_id.or_else(|| model_select(options))),
        "effort" | "thought_level" | "thinking" => (
            "effort",
            by_id.or_else(|| {
                options
                    .iter()
                    .find(|o| o.category == Some(SessionConfigOptionCategory::ThoughtLevel))
            }),
        ),
        _ => (key, by_id),
    };
    found.ok_or_else(|| {
        let ids: Vec<&str> = options.iter().map(|o| &*o.id.0).collect();
        format!(
            "this harness offers no {label} option (has: {})",
            ids.join(", ")
        )
    })
}

/// The request setting `key` to `value` (75 §8), or the refusal: a missing
/// option or a value the option does not list, each naming the list. A
/// boolean option takes `true`/`false`; a select with no listed values takes
/// anything (the harness decides).
fn resolve(
    options: &[SessionConfigOption],
    key: &str,
    value: &str,
) -> Result<(SessionConfigId, SessionConfigOptionValue, String), String> {
    let o = find_option(options, key)?;
    let view = ConfigOption::from(o);
    let values: Vec<&str> = view.options.iter().map(|v| v.value.as_str()).collect();
    let values = values.join(", ");
    let v = match &o.kind {
        SessionConfigKind::Boolean(_) => match value {
            "true" | "false" => SessionConfigOptionValue::Boolean {
                value: value == "true",
            },
            _ => {
                return Err(format!(
                    "unknown value `{value}` for {} (has: {values})",
                    view.id
                ));
            }
        },
        _ => {
            if !view.options.is_empty() && !view.options.iter().any(|v| v.value == value) {
                return Err(format!(
                    "unknown value `{value}` for {} (has: {values})",
                    view.id
                ));
            }
            SessionConfigOptionValue::value_id(SessionConfigValueId::new(value))
        }
    };
    Ok((o.id.clone(), v, values))
}

/// One `session/set_config_option` for `key = value` against `options` (see
/// [`resolve`]); the harness's error text gains the option's values. The
/// response's full option list.
async fn set_option(
    cx: &ConnectionTo<Agent>,
    session: &SessionId,
    options: &[SessionConfigOption],
    key: &str,
    value: &str,
) -> Result<Vec<SessionConfigOption>, String> {
    let (id, v, values) = resolve(options, key, value)?;
    cx.send_request(SetSessionConfigOptionRequest::new(session.clone(), id, v))
        .block_task()
        .await
        .map(|r| r.config_options)
        .map_err(|e| format!("{} (has: {values})", error_text(&e)))
}

/// Open, `initialize`, `session/new`, report, close. Never authenticates.
/// [`PROBE_TIMEOUT`] → `spawn failed: timeout`.
pub async fn probe(transport: Transport, stderr_log: PathBuf) -> ProbeResult {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let attempt = async {
        let config = transport.config().cloned();
        let mut link = Link::spawn(transport, &stderr_log, tx).await?;
        link.open(config.as_ref(), &cwd, None, None, &[], false)
            .await
            .map_err(|f| match f {
                Fail::Login(cmd) => format!("login required: {cmd}"),
                Fail::Other(why) => format!("spawn failed: {why}"),
            })
    };
    match tokio::time::timeout(PROBE_TIMEOUT, attempt).await {
        Ok(Ok(_)) => ProbeResult {
            ok: true,
            status: "ok".to_owned(),
        },
        Ok(Err(status)) => ProbeResult { ok: false, status },
        Err(_) => ProbeResult {
            ok: false,
            status: "spawn failed: timeout".to_owned(),
        },
    }
}

/// 06 §8.4: client capabilities — no `fs`, no `terminal`, `auth.terminal`,
/// no `elicitation` (theuth renders no forms; advertising `form` makes
/// opencode cancel the question), and `_meta["opencode/child-session-updates"]`
/// so opencode ≥ 2 forwards sub-agent updates (toolCallIds `<child>:<id>`).
fn initialize_request() -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            ClientCapabilities::new()
                .terminal(false)
                .auth(AuthCapabilities::new().terminal(true))
                .meta(Meta::from_iter([(
                    "opencode/child-session-updates".to_owned(),
                    serde_json::Value::Bool(true),
                )])),
        )
        .client_info(Implementation::new("theuth", env!("CARGO_PKG_VERSION")))
}

/// The command line the user must run for a terminal-type auth method: our
/// configured invocation plus the method's `args`, prefixed by its `env`.
/// Without a process (a dial transport) there is nothing to run here: the
/// method's name, or `login`.
fn login_command(config: Option<&AcpAgentConfig>, methods: &[AuthMethod]) -> String {
    let terminal = methods.iter().find_map(|m| match m {
        AuthMethod::Terminal(t) => Some(t),
        _ => None,
    });
    let Some(config) = config else {
        return terminal.map_or_else(|| "login".to_owned(), |t| t.name.clone());
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = terminal {
        parts.extend(t.env.iter().map(|(k, v)| format!("{k}={v}")));
    }
    parts.push(config.command().display().to_string());
    parts.extend(config.arguments().iter().cloned());
    if let Some(t) = terminal {
        parts.extend(t.args.iter().cloned());
    }
    parts.join(" ")
}

/// Serde name of a stop reason (`end_turn`, `cancelled`, …).
fn stop_reason(s: StopReason) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "end_turn".to_owned())
}

/// What the connection task's result says about the transport.
fn exit_text(r: Result<Result<(), Error>, tokio::task::JoinError>, closed: &str) -> String {
    match r {
        Ok(Ok(())) => closed.to_owned(),
        Ok(Err(e)) => error_text(&e),
        Err(e) => format!("connection task failed: {e}"),
    }
}

/// `message: data` for the SDK's internal errors, whose `data` is either a
/// string or `{spawned_at, data}` (the spawn failure carries the OS error
/// there); the plain `Display` otherwise.
fn error_text(e: &Error) -> String {
    let detail = match &e.data {
        Some(serde_json::Value::String(s)) => Some(s.as_str()),
        Some(serde_json::Value::Object(o)) => o.get("data").and_then(|d| d.as_str()),
        _ => None,
    };
    match detail {
        Some(d) => format!("{}: {d}", e.message),
        None => e.to_string(),
    }
}

fn open_log(path: &Path) -> Option<File> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match File::options().append(true).create(true).open(path) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!("acp stderr log {}: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{AuthMethodAgent, AuthMethodTerminal};

    #[test]
    fn stop_reasons_are_serde_names() {
        assert_eq!(stop_reason(StopReason::EndTurn), "end_turn");
        assert_eq!(
            stop_reason(StopReason::MaxTurnRequests),
            "max_turn_requests"
        );
        assert_eq!(stop_reason(StopReason::Cancelled), "cancelled");
    }

    /// omp/opencode shape: `category: model` wins over an earlier `mode`
    /// select; without categories the id/name fallback picks the model
    /// select; a boolean or unrelated option yields nothing.
    #[test]
    fn current_model_reads_the_model_select() {
        let opts: Vec<SessionConfigOption> = serde_json::from_value(serde_json::json!([
            {"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":"default",
             "options":[{"value":"default","name":"Default"}]},
            {"id":"model","name":"Model","category":"model","type":"select",
             "currentValue":"anthropic/claude-opus-4-8",
             "options":[{"value":"anthropic/claude-opus-4-8","name":"Claude Opus 4.8"}]}
        ]))
        .unwrap();
        assert_eq!(
            current_model(&opts).as_deref(),
            Some("anthropic/claude-opus-4-8")
        );
        let uncategorised: Vec<SessionConfigOption> = serde_json::from_value(serde_json::json!([
            {"id":"fast","name":"Fast mode","type":"boolean","currentValue":true},
            {"id":"llm","name":"Active model","type":"select","currentValue":"gpt-5.4",
             "options":[{"value":"gpt-5.4","name":"GPT-5.4"}]}
        ]))
        .unwrap();
        assert_eq!(current_model(&uncategorised).as_deref(), Some("gpt-5.4"));
        assert_eq!(current_model(&uncategorised[..1]), None);
        assert_eq!(current_model(&[]), None);
    }

    /// 75 §8: `effort` reaches omp's `thinking` by category; a boolean takes
    /// `true`/`false` only; grouped selects flatten into the view; missing
    /// options and unlisted values are refused naming the list.
    #[test]
    fn resolve_finds_options_by_id_or_alias_and_checks_values() {
        let opts: Vec<SessionConfigOption> = serde_json::from_value(serde_json::json!([
            {"id":"model","name":"Model","category":"model","type":"select","currentValue":"a/x",
             "options":[{"group":"a","name":"A","options":[{"value":"a/x","name":"X"}]},
                        {"group":"b","name":"B","options":[{"value":"b/y","name":"Y","description":"why"}]}]},
            {"id":"thinking","name":"Thinking","category":"thought_level","type":"select","currentValue":"medium",
             "options":[{"value":"low","name":"Low"},{"value":"medium","name":"Medium"}]},
            {"id":"fast","name":"Fast","type":"boolean","currentValue":false}
        ]))
        .unwrap();
        let view: Vec<ConfigOption> = opts.iter().map(ConfigOption::from).collect();
        assert_eq!(view[0].category.as_deref(), Some("model"));
        assert_eq!(
            view[0]
                .options
                .iter()
                .map(|v| v.value.as_str())
                .collect::<Vec<_>>(),
            ["a/x", "b/y"]
        );
        assert_eq!(view[1].category.as_deref(), Some("thought_level"));
        assert_eq!(
            (view[2].current.as_str(), view[2].category.as_deref()),
            ("false", None)
        );

        let (id, v, values) = resolve(&opts, "effort", "low").unwrap();
        assert_eq!((&*id.0, values.as_str()), ("thinking", "low, medium"));
        assert!(matches!(v, SessionConfigOptionValue::ValueId { value } if &*value.0 == "low"));
        assert!(matches!(
            resolve(&opts, "fast", "true").unwrap().1,
            SessionConfigOptionValue::Boolean { value: true }
        ));
        assert_eq!(
            resolve(&opts, "fast", "yes").unwrap_err(),
            "unknown value `yes` for fast (has: true, false)"
        );
        assert_eq!(
            resolve(&opts, "model", "c/z").unwrap_err(),
            "unknown value `c/z` for model (has: a/x, b/y)"
        );
        assert_eq!(
            resolve(&opts[..1], "effort", "low").unwrap_err(),
            "this harness offers no effort option (has: model)"
        );
        assert_eq!(
            resolve(&opts, "mode", "x").unwrap_err(),
            "this harness offers no mode option (has: model, thinking, fast)"
        );
    }

    /// A recorded frame (crates/acp/fixtures, live smoke 2026-10-03).
    fn fixture(version: &str, name: &str) -> serde_json::Value {
        let path = format!(
            "{}/fixtures/opencode-{version}/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).expect(&path)
    }

    fn options_of(result: &serde_json::Value) -> Vec<SessionConfigOption> {
        serde_json::from_value(result["configOptions"].clone()).unwrap()
    }

    /// 06 §8.4, recorded from `opencode acp` 2.0.22 and 1.18.30: `agentInfo`
    /// decides the usage semantics (2.x a turn sum, 1.x a lower bound);
    /// `session/new` announces `model` + `mode` on both, `effort` on 2.x from
    /// the start and on 1.x only once the model has variants; the model is
    /// set with `set_config_option {configId: "model"}` and the effort
    /// `high` resolves on both after that, while `medium` (a theuth effort
    /// no variant matches) is a refusal naming the list — the connect logs
    /// it and keeps the default.
    #[test]
    fn opencode_fixtures_initialize_and_config_options() {
        for (version, lower) in [("2.0.22", false), ("1.18.30", true)] {
            let init: InitializeResponse =
                serde_json::from_value(fixture(version, "initialize.result.json")).unwrap();
            let agent = init.agent_info.as_ref().unwrap();
            assert_eq!(
                (agent.name.as_str(), agent.version.as_str()),
                ("OpenCode", version)
            );
            assert_eq!(usage_is_lower_bound(Some(agent)), lower, "{version}");
            assert!(init.agent_capabilities.load_session);

            let fresh = options_of(&fixture(version, "session_new.result.json"));
            let ids: Vec<&str> = fresh.iter().map(|o| &*o.id.0).collect();
            assert!(
                ids.starts_with(&["model"]) && ids.contains(&"mode"),
                "{ids:?}"
            );
            assert_eq!(ids.contains(&"effort"), version == "2.0.22", "{version}");
            assert_ne!(
                current_model(&fresh).as_deref(),
                Some("anthropic/claude-haiku-4-5")
            );
            let (id, v, _) = resolve(&fresh, "model", "anthropic/claude-haiku-4-5").unwrap();
            assert_eq!(&*id.0, "model");
            let sent = fixture(version, "set_config_option.model.request.json");
            assert_eq!(serde_json::to_value(&v).unwrap()["value"], sent["value"]);
            assert_eq!(sent["configId"], "model");

            let switched = options_of(&fixture(version, "set_config_option.model.result.json"));
            assert_eq!(
                current_model(&switched).as_deref(),
                Some("anthropic/claude-haiku-4-5")
            );
            let (id, _, values) = resolve(&switched, "effort", "high").unwrap();
            assert_eq!(&*id.0, "effort");
            assert!(
                values.contains("high") && values.contains("max"),
                "{values}"
            );
            assert_eq!(
                resolve(&switched, "effort", "medium").unwrap_err(),
                format!("unknown value `medium` for effort (has: {values})")
            );
            assert_eq!(current_value(&switched, "mode").as_deref(), Some("build"));
        }
        // 2.x asked for `high` from `default`; 1.x already sat on `high`
        // after the model switch, so no request went out (see connect).
        let after = options_of(&fixture("2.0.22", "set_config_option.effort.result.json"));
        assert_eq!(current_value(&after, "effort").as_deref(), Some("high"));
        assert_eq!(
            fixture("2.0.22", "set_config_option.effort.request.json")["value"],
            "high"
        );
        let v1 = options_of(&fixture("1.18.30", "set_config_option.model.result.json"));
        assert_eq!(current_value(&v1, "effort").as_deref(), Some("high"));
    }

    /// 06 §8.4: 2.x forwards a sub-agent's updates on
    /// `opencode/session/child_update`; the tool call keeps its `<child>:<id>`
    /// id and `_meta`, the sub-agent's prose becomes thought, a `status`
    /// event and other methods are nothing.
    #[test]
    fn child_update_fixtures_become_session_updates() {
        let as_untyped =
            |v: serde_json::Value| -> UntypedMessage { serde_json::from_value(v).unwrap() };
        let n = as_untyped(fixture("2.0.22", "child_update.tool_call.json"));
        assert_eq!(n.method, CHILD_UPDATE_METHOD);
        let Some(SessionUpdate::ToolCall(tc)) = child_update(&n) else {
            panic!("tool_call expected");
        };
        assert_eq!(
            &*tc.tool_call_id.0,
            "ses_f00417af0ffewZH4KxaHpEZ653:toolu_0179KkBvErCkm8J5u2hVjtLG"
        );
        assert_eq!(tc.title, "Create hello.txt file: write");
        assert!(
            tc.meta
                .as_ref()
                .is_some_and(|m| m.contains_key("opencode/child-session"))
        );

        let n = as_untyped(fixture(
            "2.0.22",
            "child_update.tool_call_update.completed.json",
        ));
        let Some(SessionUpdate::ToolCallUpdate(u)) = child_update(&n) else {
            panic!("tool_call_update expected");
        };
        assert_eq!(
            u.fields.status,
            Some(agent_client_protocol::schema::v1::ToolCallStatus::Completed)
        );
        assert_eq!(
            u.fields.locations.as_deref().map(|l| l.len()),
            Some(1),
            "the written file's location"
        );

        let n = as_untyped(fixture("2.0.22", "child_update.agent_message_chunk.json"));
        assert!(matches!(
            child_update(&n),
            Some(SessionUpdate::AgentThoughtChunk(_))
        ));

        let status = as_untyped(
            serde_json::json!({"jsonrpc": "2.0", "method": CHILD_UPDATE_METHOD,
            "params": {"rootSessionId": "r", "childSessionId": "c", "parentSessionId": "r", "depth": 1,
                       "type": "status", "status": "completed"}}),
        );
        assert!(child_update(&status).is_none());
        let other = UntypedMessage::new("session/update", serde_json::json!({})).unwrap();
        assert!(child_update(&other).is_none());
    }

    /// 2.x permission asks carry `diff` content and `locations` (06 §8.4)
    /// and may name a sub-agent's call; option ids are unchanged since 1.x.
    #[test]
    fn permission_fixtures_deserialize_with_diff_and_locations() {
        let v2 = fixture("2.0.22", "request_permission.diff.json");
        let r: RequestPermissionRequest = serde_json::from_value(v2["params"].clone()).unwrap();
        assert!(r.tool_call.tool_call_id.0.contains(':'));
        assert_eq!(
            r.tool_call.fields.kind,
            Some(agent_client_protocol::schema::v1::ToolKind::Edit)
        );
        let content = r.tool_call.fields.content.as_deref().unwrap();
        assert!(matches!(
            content[0],
            agent_client_protocol::schema::v1::ToolCallContent::Diff { .. }
        ));
        assert_eq!(
            r.tool_call.fields.locations.as_deref().unwrap()[0].path,
            Path::new("/tmp/oc2-smoke/work/hello.txt")
        );
        for version in ["2.0.22", "1.18.30"] {
            let v = fixture(version, "request_permission.execute.json");
            let r: RequestPermissionRequest = serde_json::from_value(v["params"].clone()).unwrap();
            let ids: Vec<&str> = r.options.iter().map(|o| &*o.option_id.0).collect();
            assert_eq!(ids, ["once", "always", "reject"], "{version}");
            assert_eq!(
                r.tool_call.fields.kind,
                Some(agent_client_protocol::schema::v1::ToolKind::Execute)
            );
        }
    }

    /// The recorded `PromptResponse.usage` of both versions deserializes with
    /// the unstable feature on and sums to `totalTokens`; `usage_update`
    /// carries `used`/`size`/`cost` on both.
    #[test]
    fn usage_fixtures_deserialize() {
        use agent_client_protocol::schema::v1::{PromptResponse, UsageUpdate};
        for version in ["2.0.22", "1.18.30"] {
            let r: PromptResponse =
                serde_json::from_value(fixture(version, "prompt.result.json")).unwrap();
            assert_eq!(stop_reason(r.stop_reason), "end_turn");
            let u = r.usage.unwrap();
            assert_eq!(
                u.input_tokens
                    + u.output_tokens
                    + u.thought_tokens.unwrap_or(0)
                    + u.cached_read_tokens.unwrap_or(0)
                    + u.cached_write_tokens.unwrap_or(0),
                u.total_tokens,
                "{version}"
            );
            let n: SessionNotification = serde_json::from_value(
                fixture(version, "usage_update.notification.json")["params"].clone(),
            )
            .unwrap();
            let SessionUpdate::UsageUpdate(UsageUpdate {
                used, size, cost, ..
            }) = n.update
            else {
                panic!("usage_update expected");
            };
            assert!(used > 0 && size == 200_000, "{version}: {used}/{size}");
            assert_eq!(cost.unwrap().currency, "USD");
        }
    }

    #[test]
    fn login_command_appends_terminal_method() {
        let config = AcpAgentConfig::new("codex-acp").arg("--acp");
        let mut term = AuthMethodTerminal::new("login", "Log in");
        term.args = vec!["--login".into()];
        term.env = [("A".to_owned(), "1".to_owned())].into();
        let methods = vec![
            AuthMethod::Agent(AuthMethodAgent::new("oauth", "OAuth")),
            AuthMethod::Terminal(term),
        ];
        assert_eq!(
            login_command(Some(&config), &methods),
            "A=1 codex-acp --acp --login"
        );
        assert_eq!(login_command(Some(&config), &[]), "codex-acp --acp");
        assert_eq!(login_command(None, &methods), "Log in");
        assert_eq!(login_command(None, &[]), "login");
    }

    /// A dialer that fails never reaches the SDK: no process, the dial text is the error.
    #[derive(Debug)]
    struct Offline;

    #[async_trait::async_trait]
    impl Dialer for Offline {
        async fn dial(&self) -> Result<LineDuplex, String> {
            Err("offline".to_owned())
        }
    }

    #[tokio::test]
    async fn dial_failure_is_the_connect_error() {
        let dir = std::env::temp_dir().join(format!("theuth-acp-dial-{}", std::process::id()));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let r = Harness::connect(
            Connect {
                transport: Transport::Dial(Arc::new(Offline)),
                cwd: dir.clone(),
                resume: None,
                stderr_log: dir.join("acp-offline.log"),
                meta: None,
                model: None,
                effort: None,
                mcp_servers: Vec::new(),
            },
            tx,
        )
        .await;
        assert!(r.as_ref().is_err_and(|e| e.contains("offline")), "{r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
