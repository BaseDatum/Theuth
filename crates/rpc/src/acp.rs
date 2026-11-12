//! The agent half of the Agent Client Protocol (design doc 06 §4, §8.12-13):
//! `t --acp` on stdio, and one connection per link mode-1 conn when this
//! harness is a node (32 §4.3, §5.3). Every call is an [`api::clients`] method
//! on an [`Engine`] — this process's, or a running one `t --acp` attached to
//! (63 §2.3) — with the ACP client (an editor, or a parent harness) as the
//! only frontend.
//!
//! One kameo actor per process, [`Relay`] (`"acp.relay"`), is subscribed to
//! the bus: it turns [`AgentEvent`]s into `session/update` notifications on
//! the connection that owns the session, answers `session/prompt` when the
//! session's `TurnFinished` arrives, and forwards [`ApprovalRequested`] as
//! `session/request_permission`. The theuth [`SessionId`] string *is* the ACP
//! session id.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agent::cost::{Coverage, NANOS_PER_UNIT, SessionCost};
use agent::todo::{Status, Todo};
use agent::{
    AgentEvent, CreateSessionOptions, HistoryEntry, PeerDelivered, PeerUplink, RegisterNode,
    UnregisterNode,
};
use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, Cost,
    EmbeddedResourceResource, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, PromptCapabilities, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, ResourceLink, SessionId as AcpSessionId,
    SessionInfo, SessionNotification, SessionUpdate, StopReason, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, UsageUpdate,
};
use agent_client_protocol::{Agent, Client, ConnectTo, ConnectionTo, Error, Lines, Stdio};
use artifact::ArtifactShown;
use base64::Engine as _;
use jsonrpsee::core::client::Error as ClientError;
use jsonrpsee::ws_client::WsClient;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use tokio::sync::{broadcast, mpsc, oneshot};
use tools::{ApprovalRequested, Decision, Risk};

use crate::api::{self, clients::*};
use crate::client::{Engine, Local};

/// Registry name of the process-wide [`Relay`].
pub const ACP_RELAY_ACTOR: &str = "acp.relay";

/// The plugin: ACP on stdin/stdout until the client closes stdin, then
/// `rt.shutdown`. Nothing else may write to stdout in this mode.
pub struct AcpPlugin {
    /// Attached to a running engine (63 §2.3): its socket; `None` when this
    /// process is the engine. Events arrive through [`crate::client::mirror`];
    /// the socket closing ends the process (the editor restarts it).
    pub remote: Option<Arc<WsClient>>,
}

#[async_trait::async_trait]
impl runtime::Plugin for AcpPlugin {
    fn name(&self) -> &'static str {
        "acp"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let api = Arc::new(match &self.remote {
            Some(ws) => Engine::Remote(ws.clone()),
            None => Engine::Local(Local::new(rt.clone())),
        });
        let relay = relay(rt, Some(api.clone())).await;
        if let Some(ws) = self.remote {
            let (rt, shutdown) = (rt.clone(), rt.shutdown.clone());
            tokio::spawn(async move {
                if let Err(e) = crate::client::mirror(&rt, &ws).await {
                    tracing::warn!(error = %e, "acp: engine event stream");
                }
                tracing::info!("acp: engine gone; exiting");
                shutdown.cancel();
            });
        }
        let shutdown = rt.shutdown.clone();
        let data_dir = rt.data_dir.clone();
        tokio::spawn(async move {
            if let Err(e) = serve(Stdio::new(), relay, api, data_dir).await {
                tracing::error!(error = %e, "acp connection failed");
            }
            shutdown.cancel();
        });
        Ok(())
    }
}

/// The process-wide [`Relay`]: the registered actor, or a fresh one spawned
/// on [`runtime::subscriber_mailbox`], subscribed to the bus and registered
/// as [`ACP_RELAY_ACTOR`]. Whichever caller comes first creates it, on `api`
/// (default: this process's engine).
pub async fn relay(rt: &Runtime, api: Option<Arc<Engine>>) -> ActorRef<Relay> {
    if let Ok(Some(relay)) = ActorRef::<Relay>::lookup(ACP_RELAY_ACTOR) {
        return relay;
    }
    let api = api.unwrap_or_else(|| Arc::new(Engine::Local(Local::new(rt.clone()))));
    let relay = Relay::spawn_with_mailbox(
        Relay {
            api,
            connections: HashMap::new(),
            owners: HashMap::new(),
            waiters: HashMap::new(),
        },
        runtime::subscriber_mailbox(),
    );
    if let Err(e) = relay.register(ACP_RELAY_ACTOR) {
        tracing::warn!(error = %e, "acp.relay already registered; two relays");
    }
    rt.subscribe::<Relay, AgentEvent>(&relay).await;
    rt.subscribe::<Relay, ApprovalRequested>(&relay).await;
    rt.subscribe::<Relay, ArtifactShown>(&relay).await;
    rt.subscribe::<Relay, PeerDelivered>(&relay).await;
    rt.subscribe::<Relay, PeerUplink>(&relay).await;
    relay
}

/// ACP agent over one link mode-1 conn (32 §4.3, §5.3): `inbound` yields one
/// reassembled JSON-RPC message per item, `outbound` takes one per item, no
/// line framing. Returns when either half closes; never touches
/// `rt.shutdown`.
pub async fn serve_records(
    rt: Runtime,
    inbound: mpsc::Receiver<Vec<u8>>,
    outbound: mpsc::Sender<Vec<u8>>,
) {
    let incoming = futures::stream::unfold(inbound, async |mut rx: mpsc::Receiver<Vec<u8>>| {
        let line = String::from_utf8(rx.recv().await?)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
        Some((line, rx))
    });
    let outgoing =
        futures::sink::unfold(outbound, async |tx: mpsc::Sender<Vec<u8>>, line: String| {
            tx.send(line.into_bytes())
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            Ok::<_, io::Error>(tx)
        });
    let relay = relay(&rt, None).await;
    let api = relay
        .ask(Api)
        .await
        .unwrap_or_else(|_| Arc::new(Engine::Local(Local::new(rt.clone()))));
    if let Err(e) = serve(
        Lines::new(outgoing, incoming),
        relay,
        api,
        rt.data_dir.clone(),
    )
    .await
    {
        tracing::debug!(error = %e, "acp conn ended with error");
    }
}

/// 67 §4.2, decision 4: the same ACP over the mesh. Two loops until
/// shutdown, once the mesh has registered its transport: every inbound
/// `theuth/acp/1` stream is one [`serve`] connection (as a link mode-1 conn
/// is), and every paired engine is registered with `agent` as a node
/// ([`RegisterNode`] with an [`acp::MeshDialer`]), so `agent: "build-A"` /
/// `"omp@build-A"` and the tool description work as for a 32 node. A peer
/// is re-registered on every connect/disconnect (`· offline` while away,
/// like a node); one removed from the mesh is unregistered.
pub async fn serve_mesh(rt: Runtime) {
    let Some(transport) = replica::transport(&rt).await else {
        return;
    };
    // 68 §5: forwarded session RPCs from other front doors, on `theuth/rpc/1`.
    tokio::spawn(crate::forward::serve(
        rt.clone(),
        transport.clone(),
        crate::methods(rt.clone()).await,
    ));
    let relay = relay(&rt, None).await;
    let api = relay
        .ask(Api)
        .await
        .unwrap_or_else(|_| Arc::new(Engine::Local(Local::new(rt.clone()))));
    let (t, data_dir, shutdown) = (transport.clone(), rt.data_dir.clone(), rt.shutdown.clone());
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                () = shutdown.cancelled() => return,
                a = t.accept(replica::ALPN_ACP) => a,
            };
            match accepted {
                Ok((peer, stream)) => {
                    let (relay, api, data_dir) = (relay.clone(), api.clone(), data_dir.clone());
                    tokio::spawn(async move {
                        let d = acp::line_duplex(stream);
                        if let Err(e) =
                            serve(Lines::new(d.outgoing, d.incoming), relay, api, data_dir).await
                        {
                            tracing::debug!(%peer, error = %e, "acp mesh conn ended with error");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "acp mesh accept");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    });
    let (Some(mesh), Some(agents)) = tokio::join!(
        rt.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR),
        rt.lookup_when::<agent::Agents>(agent::AGENT_ACTOR),
    ) else {
        return;
    };
    let Ok(mut events) = mesh.ask(mesh::Subscribe).await else {
        return;
    };
    let mut registered: HashMap<String, String> = HashMap::new(); // id → node name
    loop {
        if let Ok(status) = mesh.ask(mesh::Status).await {
            let stats = mesh.ask(mesh::ListStats).await.unwrap_or_default();
            let mut seen = HashMap::new();
            for p in status.peers {
                let name = node_name(&p, &seen);
                seen.insert(p.id.0.clone(), name.clone());
                let describe = describe(&p, stats.iter().find(|s| s.id == p.id));
                let _ = agents
                    .ask(RegisterNode {
                        name: name.clone(),
                        describe,
                        dialer: Arc::new(acp::MeshDialer {
                            name,
                            peer: p.id.into(),
                            self_id: status.self_id.0.clone(),
                            transport: transport.clone(),
                        }),
                    })
                    .await;
            }
            for (id, name) in &registered {
                if !seen.contains_key(id) {
                    let _ = agents.ask(UnregisterNode { name: name.clone() }).await;
                }
            }
            registered = seen;
        }
        loop {
            match events.recv().await {
                Ok(mesh::MeshEvent::Peer(_)) | Err(broadcast::error::RecvError::Lagged(_)) => break,
                Ok(
                    mesh::MeshEvent::Lease(_)
                    | mesh::MeshEvent::Nearby(_)
                    | mesh::MeshEvent::Pair(_),
                ) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

/// The node name a mesh peer registers under: its `mesh.name`, or its id
/// prefix when it has none; suffixed with the id prefix when another peer
/// already took that name this pass (two engines with one hostname).
fn node_name(p: &mesh::Peer, taken: &HashMap<String, String>) -> String {
    let short = p.id.0.get(..8).unwrap_or(&p.id.0);
    let name = p.name.trim();
    if name.is_empty() {
        return short.to_owned();
    }
    if taken.values().any(|n| n == name) {
        format!("{name}-{short}")
    } else {
        name.to_owned()
    }
}

/// The node's `describe` line: `mesh · <class> · harnesses: a, b`, `mesh · offline`
/// while unreachable, bare `mesh` before its stats row arrives.
fn describe(p: &mesh::Peer, s: Option<&mesh::EngineStats>) -> String {
    match s {
        _ if !p.reachable => "mesh · offline".to_owned(),
        Some(s) => format!(
            "mesh · {} · harnesses: {}",
            format!("{:?}", s.class).to_lowercase(),
            s.harnesses.join(", ")
        ),
        None => "mesh".to_owned(),
    }
}

/// Ids handed to each [`serve`] connection so the [`Relay`] can tell them
/// apart and forget one when it ends.
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// Serve ACP on `transport`; returns when the peer closes its half or the
/// transport fails. `data_dir` hosts `attachments/` for prompt images (17 §5).
/// Sessions this connection creates or loads are bound to it on `relay`;
/// they are unbound when it ends.
async fn serve(
    transport: impl ConnectTo<Agent> + 'static,
    relay: ActorRef<Relay>,
    api: Arc<Engine>,
    data_dir: PathBuf,
) -> Result<(), Error> {
    let id = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
    let for_new = relay.clone();
    let for_load = relay.clone();
    let for_prompt = relay.clone();
    let (api_new, api_load, api_list, api_prompt, api_cancel) =
        (api.clone(), api.clone(), api.clone(), api.clone(), api);
    let result = Agent
        .builder()
        .name("theuth")
        .on_receive_request(
            async move |req: InitializeRequest, responder, _cx| {
                responder.respond(
                    InitializeResponse::new(req.protocol_version)
                        .agent_capabilities(
                            AgentCapabilities::new()
                                .load_session(true)
                                .prompt_capabilities(
                                    PromptCapabilities::new().embedded_context(true).image(true),
                                ),
                        )
                        .agent_info(Implementation::new("theuth", env!("CARGO_PKG_VERSION"))),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, cx| {
                let options = new_session_options(&req)?;
                let remote_harness = options.harness.is_some();
                let session = api_new
                    .create_session(options)
                    .await
                    .map_err(|e| {
                        let mut e = rpc_error(e);
                        // 67 §8: the parent asked for *this* node's harness;
                        // say which node is answering and what it has.
                        if remote_harness {
                            e.message = format!("{}: {}", platform::process::hostname(), e.message);
                        }
                        e
                    })?
                    .session;
                let _ = for_new.tell(Bind { session, id, cx }).await;
                responder.respond(NewSessionResponse::new(session.to_string()))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: LoadSessionRequest, responder, cx| {
                let session = parse_session(&req.session_id)?;
                let loaded = api_load
                    .load_session(session, None, None, Some(false))
                    .await
                    .map_err(rpc_error)?;
                for (i, entry) in loaded.history.into_iter().enumerate() {
                    cx.send_notification(SessionNotification::new(
                        req.session_id.clone(),
                        replay(i, entry),
                    ))?;
                }
                let _ = for_load.tell(Bind { session, id, cx }).await;
                responder.respond(LoadSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: ListSessionsRequest, responder, _cx| {
                let sessions = api_list.list_sessions().await.map_err(rpc_error)?;
                let here = std::env::current_dir().unwrap_or_default();
                let infos = sessions
                    .into_iter()
                    .map(|s| {
                        let cwd = s.cwd.map_or_else(|| here.clone(), PathBuf::from);
                        SessionInfo::new(s.id.to_string(), cwd)
                            .title(s.title)
                            .updated_at(s.updated_at.to_rfc3339())
                    })
                    .filter(|s| req.cwd.as_ref().is_none_or(|want| *want == s.cwd))
                    .collect();
                responder.respond(ListSessionsResponse::new(infos))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: PromptRequest, responder, cx| {
                let session = parse_session(&req.session_id)?;
                let (text, files) = prompt_parts(&req.prompt, &data_dir);
                // Register before `SendInput` so the bus cannot deliver
                // `TurnFinished` ahead of the waiter (one ordered mailbox).
                let (tx, rx) = oneshot::channel();
                let _ = for_prompt.tell(Wait { session, tx }).await;
                api_prompt
                    .send_input(Some(session), text, files, None, None)
                    .await
                    .map_err(rpc_error)?;
                cx.spawn(async move {
                    // A dropped sender means the connection is going away.
                    let stop = rx.await.unwrap_or_default();
                    let _ = responder.respond(PromptResponse::new(stop_reason(stop)));
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |n: CancelNotification, _cx| {
                let session = parse_session(&n.session_id)?;
                api_cancel
                    .cancel(session, None, None)
                    .await
                    .map_err(rpc_error)
            },
            agent_client_protocol::on_receive_notification!(),
        )
        // 141 §4.4 (R2A §5.3): `connect_to` waits for the owned `Stdio`
        // driver, so the last stdout write is flushed at stdin EOF;
        // `connect_with(.., incoming_closed)` on SDK 3.1 would not.
        .connect_to(transport)
        .await;
    let _ = relay.tell(Unbind { id }).await;
    result
}

/// A JSON-RPC error from [`crate::api`] as an ACP error (same code and
/// text); a transport failure is an internal error with its text.
fn rpc_error(e: ClientError) -> Error {
    match e {
        ClientError::Call(e) => Error::new(e.code(), e.message()),
        e => Error::new(-32000, e.to_string()),
    }
}

/// `session/new` as a `create_session`: the client's cwd, plus what a parent
/// theuth put under `_meta.theuth` (67 §8) — its model and effort pick for
/// this root, its `tools` ceiling, who it is, and the `harness` of ours to
/// run instead of a native session. No `_meta` → exactly the plain create.
/// A malformed `theuth` entry is an invalid-params error, not a silent default.
fn new_session_options(req: &NewSessionRequest) -> Result<CreateSessionOptions, Error> {
    let meta = acp::SessionMeta::from_meta(req.meta.as_ref())
        .map_err(|e| Error::invalid_params().data(e))?
        .unwrap_or_default();
    Ok(CreateSessionOptions {
        cwd: Some(req.cwd.to_string_lossy().into_owned()),
        model: meta.model,
        effort: meta.effort,
        cap: meta.tools.filter(|r| *r != Risk::Destructive),
        parent_session: meta.parent,
        parent_engine: meta.parent_engine,
        harness: meta.harness,
        ..Default::default()
    })
}

/// The theuth session behind an ACP session id.
fn parse_session(id: &AcpSessionId) -> Result<SessionId, Error> {
    id.0.parse()
        .map_err(|_| Error::invalid_params().data(format!("unknown session {id}")))
}

/// `session/prompt` content as one user message: text verbatim, embedded text
/// resources fenced with their uri, links as their uri; image blocks and PDF
/// blob resources decoded to files under `<data_dir>/attachments/` and
/// returned as paths (17 §5, 20 §2.5).
fn prompt_parts(blocks: &[ContentBlock], data_dir: &Path) -> (String, Vec<PathBuf>) {
    let mut out = String::new();
    let mut files = Vec::new();
    for block in blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        match block {
            ContentBlock::Text(t) => out.push_str(&t.text),
            ContentBlock::ResourceLink(l) => out.push_str(&l.uri),
            ContentBlock::Resource(r) => match &r.resource {
                EmbeddedResourceResource::TextResourceContents(t) => {
                    let _ = write!(out, "```{}\n{}\n```", t.uri, t.text);
                }
                EmbeddedResourceResource::BlobResourceContents(b) => {
                    match save_blob(&b.blob, b.mime_type.as_deref().unwrap_or(""), data_dir) {
                        Ok(Some(path)) => {
                            out.pop();
                            files.push(path);
                        }
                        Ok(None) => {
                            let _ = write!(out, "[binary resource {}]", b.uri);
                        }
                        Err(e) => {
                            let _ = write!(out, "[binary resource {}: {e}]", b.uri);
                        }
                    }
                }
                _ => out.push_str("[resource]"),
            },
            ContentBlock::Image(img) => match save_blob(&img.data, &img.mime_type, data_dir) {
                Ok(Some(path)) => {
                    // No text for this block: drop the separator pushed above.
                    out.pop();
                    files.push(path);
                }
                Ok(None) => {
                    let _ = write!(out, "[image: unsupported {}]", img.mime_type);
                }
                Err(e) => {
                    let _ = write!(out, "[image: {e}]");
                }
            },
            ContentBlock::Audio(_) => out.push_str("[audio]"),
            _ => out.push_str("[unsupported content]"),
        }
    }
    (out, files)
}

/// Decodes a base64 image block or PDF blob into
/// `<data_dir>/attachments/<sha256[..16]>.<ext>` ([`api::save_blob`]); `None`
/// for a mime type outside png/jpeg/gif/webp/pdf.
fn save_blob(data: &str, mime: &str, data_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        _ => return Ok(None),
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    Ok(Some(api::save_blob(&bytes, ext, data_dir)?))
}

/// theuth's stop reason string as ACP's: matching serde names pass through,
/// anything else (`error`, provider-specific) is `end_turn`.
fn stop_reason(s: String) -> StopReason {
    serde_json::from_value(serde_json::Value::String(s)).unwrap_or(StopReason::EndTurn)
}

/// 51 §8.3: outbound ACP cost carries one currency and no coverage label, so
/// it is emitted only for a complete, single-currency returned total.
fn acp_cost(c: &SessionCost) -> Option<Cost> {
    let [m] = c.total.returned.as_slice() else {
        return None;
    };
    (c.total.unavailable == 0
        && c.total.unresolved == 0
        && c.remote_meter_gaps == 0
        && c.history_coverage == Coverage::Complete)
        .then(|| Cost::new(m.nanos as f64 / NANOS_PER_UNIT as f64, m.currency.clone()))
}

/// One stored history entry as a `session/load` replay update. Rows removed
/// from context (48 §2.2) keep their original text, prefixed with a marker;
/// ACP clients have no structured tombstone.
fn replay(i: usize, entry: HistoryEntry) -> SessionUpdate {
    let removed = |text: String, context: &Option<agent::ContextEntry>| {
        if context.as_ref().is_some_and(|c| c.removed) {
            format!("[removed from context] {text}")
        } else {
            text
        }
    };
    match entry {
        HistoryEntry::User { text, context, .. } => {
            SessionUpdate::UserMessageChunk(ContentChunk::new(removed(text, &context).into()))
        }
        HistoryEntry::Assistant { text, context, .. } => {
            SessionUpdate::AgentMessageChunk(ContentChunk::new(removed(text, &context).into()))
        }
        HistoryEntry::Reasoning { text, .. } => {
            SessionUpdate::AgentThoughtChunk(ContentChunk::new(text.into()))
        }
        // A `show` / `app` entry replays as a plain tool card; the artifact
        // link it carried live (`ArtifactShown`) is a frontend join on
        // `list_artifacts` by `call_id`, any kind, not part of the history (35 §0.3).
        HistoryEntry::Tool {
            name, args, result, ..
        } => {
            let (status, output) = match result {
                Some((out, true)) => (ToolCallStatus::Failed, Some(out)),
                Some((out, false)) => (ToolCallStatus::Completed, Some(out)),
                None => (ToolCallStatus::InProgress, None),
            };
            let call = ToolCall::new(format!("h{i}"), name.clone())
                .kind(acp::kind(&name, Risk::Mutating))
                .status(status)
                .raw_input(args);
            SessionUpdate::ToolCall(match output {
                Some(out) => call.raw_output(serde_json::Value::String(out)),
                None => call,
            })
        }
        HistoryEntry::Compacted { summary, .. } => SessionUpdate::AgentMessageChunk(
            ContentChunk::new(format!("[compacted] {summary}").into()),
        ),
        HistoryEntry::Error { text, .. } => {
            SessionUpdate::AgentMessageChunk(ContentChunk::new(format!("error: {text}").into()))
        }
        HistoryEntry::Interrupted { text, .. } => SessionUpdate::AgentMessageChunk(
            ContentChunk::new(format!("[interrupted — resuming] {text}").into()),
        ),
        // 125 C: the advisor's notes replay as one user-side chunk per card.
        HistoryEntry::Advisory { advisory, .. } => {
            SessionUpdate::UserMessageChunk(ContentChunk::new(
                advisory
                    .notes
                    .iter()
                    .map(|n| format!("[advisor {:?}] {}", n.severity, n.note))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into(),
            ))
        }
        // 183 §5.3: the moved notice over ACP.
        HistoryEntry::Moved { moved, .. } => SessionUpdate::UserMessageChunk(ContentChunk::new(
            format!("[session moved to {}]", moved.to).into(),
        )),
    }
}

/// Bus subscriber routing each session's events to the ACP connection that
/// created or loaded it (32 §5.3), and holding the prompt waiters.
#[derive(Actor)]
pub struct Relay {
    /// Where its calls go (63 §2.3).
    api: Arc<Engine>,
    /// Live connections by [`serve`] id.
    connections: HashMap<u64, ConnectionTo<Client>>,
    /// The connection that owns each session; events for a session with no
    /// owner (its connection ended, or it was started elsewhere) are dropped.
    owners: HashMap<SessionId, u64>,
    /// `session/prompt` calls waiting for their session's `TurnFinished`.
    waiters: HashMap<SessionId, oneshot::Sender<String>>,
}

impl Relay {
    fn connection(&self, session: &SessionId) -> Option<&ConnectionTo<Client>> {
        self.connections.get(self.owners.get(session)?)
    }

    fn notify(&self, session: SessionId, update: SessionUpdate) {
        let Some(cx) = self.connection(&session) else {
            return;
        };
        let n = SessionNotification::new(AcpSessionId::new(session.to_string()), update);
        if let Err(e) = cx.send_notification(n) {
            tracing::warn!(error = %e, "acp session/update failed");
        }
    }
}

/// `ask` → the relay's [`Engine`], for a later connection ([`serve_records`]).
struct Api;

impl Message<Api> for Relay {
    type Reply = Arc<Engine>;
    async fn handle(&mut self, _: Api, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.api.clone()
    }
}

/// `session` now belongs to connection `id` (`session/new` or `session/load`).
struct Bind {
    session: SessionId,
    id: u64,
    cx: ConnectionTo<Client>,
}

impl Message<Bind> for Relay {
    type Reply = ();
    async fn handle(&mut self, m: Bind, _: &mut Context<Self, ()>) {
        self.connections.entry(m.id).or_insert(m.cx);
        self.owners.insert(m.session, m.id);
    }
}

/// Connection `id` ended: forget it, its sessions and their waiters.
struct Unbind {
    id: u64,
}

impl Message<Unbind> for Relay {
    type Reply = ();
    async fn handle(&mut self, m: Unbind, _: &mut Context<Self, ()>) {
        self.connections.remove(&m.id);
        self.owners.retain(|_, owner| *owner != m.id);
        self.waiters
            .retain(|session, _| self.owners.contains_key(session));
    }
}

/// Resolve `tx` with the stop reason of `session`'s next `TurnFinished`.
struct Wait {
    session: SessionId,
    tx: oneshot::Sender<String>,
}

impl Message<Wait> for Relay {
    type Reply = ();
    async fn handle(&mut self, m: Wait, _: &mut Context<Self, ()>) {
        self.waiters.insert(m.session, m.tx);
    }
}

impl Message<AgentEvent> for Relay {
    type Reply = ();
    async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, ()>) {
        match ev {
            AgentEvent::TextDelta { session, text, .. } => self.notify(
                session,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(&*text))),
            ),
            AgentEvent::ReasoningDelta { session, text, .. } => self.notify(
                session,
                SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from(&*text))),
            ),
            AgentEvent::ToolCallStarted {
                session,
                call_id,
                name,
                args,
                ..
            } => self.notify(
                session,
                SessionUpdate::ToolCall(
                    ToolCall::new(call_id, name.clone())
                        // The registry's risk is not on the event; every
                        // non-read name renders as its 06 §4.1 kind anyway.
                        .kind(acp::kind(&name, Risk::Mutating))
                        .status(ToolCallStatus::InProgress)
                        .raw_input(args),
                ),
            ),
            AgentEvent::ToolCallFinished {
                session,
                call_id,
                output,
                is_error,
                ..
            } => self.notify(
                session,
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    call_id,
                    ToolCallUpdateFields::new()
                        .status(if is_error {
                            ToolCallStatus::Failed
                        } else {
                            ToolCallStatus::Completed
                        })
                        .content(vec![ToolCallContent::from(output)]),
                )),
            ),
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                context,
                ..
            } => {
                // ponytail: window size unknown here (the catalog lives in
                // `models`); 0 until a client needs the ratio.
                let mut usage = UsageUpdate::new(u64::from(context), 0);
                // 51 §8.3: the owner's total, only when one currency says it all.
                // `Err` = no ledger for this session (51 §8.3): nothing to say.
                if let Ok(c) = self.api.session_cost(session).await
                    && let Some(cost) = acp_cost(&c)
                {
                    usage = usage.cost(cost);
                }
                self.notify(session, SessionUpdate::UsageUpdate(usage));
                if let Some(tx) = self.waiters.remove(&session) {
                    let _ = tx.send(stop_reason);
                }
            }
            AgentEvent::MessageDelivered { session, text, .. } => self.notify(
                session,
                SessionUpdate::UserMessageChunk(ContentChunk::new(text.into())),
            ),
            AgentEvent::Error {
                session, message, ..
            } => self.notify(
                session,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    format!("error: {message}").into(),
                )),
            ),
            // 47 §5.1: the plan is state, not transcript; the client sees the
            // whole list on every change. Never touches the prompt waiter.
            AgentEvent::TodoChanged { session, snapshot } => {
                self.notify(session, SessionUpdate::Plan(plan(&snapshot.items)));
            }
            _ => {}
        }
    }
}

/// A theuth todo list as an ACP plan, order kept. Pending / in progress / done
/// map directly. ACP has no blocked or dropped status: blocked stays pending
/// with the reason in the text, dropped is completed but says so — a client
/// must not read a dropped item as work done. Priority is not tracked here.
fn plan(items: &[Todo]) -> Plan {
    Plan::new(
        items
            .iter()
            .map(|t| {
                let (content, status) = match t.status {
                    Status::Pending => (t.text.clone(), PlanEntryStatus::Pending),
                    Status::InProgress => (t.text.clone(), PlanEntryStatus::InProgress),
                    Status::Done => (t.text.clone(), PlanEntryStatus::Completed),
                    Status::Blocked => (
                        match &t.reason {
                            Some(r) => format!("{} (blocked: {r})", t.text),
                            None => format!("{} (blocked)", t.text),
                        },
                        PlanEntryStatus::Pending,
                    ),
                    Status::Dropped => {
                        (format!("{} (dropped)", t.text), PlanEntryStatus::Completed)
                    }
                };
                PlanEntry::new(content, PlanEntryPriority::Medium, status)
            })
            .collect(),
    )
}

/// A shown artifact becomes a second update on its `show` tool call: one
/// `ResourceLink` the client renders as an openable file (35 §8). An app
/// record (46 §8.2) is the same link to its `.openui` source on the `app`
/// call; ACP gets no pretend controls and the source is inert there.
impl Message<ArtifactShown> for Relay {
    type Reply = ();
    async fn handle(&mut self, ev: ArtifactShown, _: &mut Context<Self, ()>) {
        let a = ev.artifact;
        let link = ResourceLink::new(a.name, format!("file://{}", a.path.display()))
            .title(a.title)
            .mime_type(a.mime)
            .size(i64::try_from(a.bytes).ok());
        self.notify(
            ev.session,
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ev.call_id,
                ToolCallUpdateFields::new().content(vec![ToolCallContent::from(
                    ContentBlock::ResourceLink(link),
                )]),
            )),
        );
    }
}

/// A peer exchange consumed into a session's transcript (50 §10) is user
/// input the ACP client did not type: shown as a user chunk once, from the
/// typed event alone (`MessageDelivered` no longer echoes peer input).
impl Message<PeerDelivered> for Relay {
    type Reply = ();
    async fn handle(&mut self, ev: PeerDelivered, _: &mut Context<Self, ()>) {
        self.notify(
            ev.session,
            SessionUpdate::UserMessageChunk(ContentChunk::new(ev.text.into())),
        );
    }
}

/// 67 §8: a root a remote theuth opened sent `send_message{to: "parent"}`;
/// out it goes on the conn that owns the session, as the
/// [`acp::PEER_MESSAGE_METHOD`] extension notification. No owner (the conn
/// ended) → dropped, like any other event for an unowned session.
impl Message<PeerUplink> for Relay {
    type Reply = ();
    async fn handle(&mut self, m: PeerUplink, _: &mut Context<Self, ()>) {
        let Some(cx) = self.connection(&m.session) else {
            return;
        };
        let n = acp::PeerMessage {
            session_id: m.session.to_string(),
            id: m.id,
            reply_to: m.reply_to,
            text: m.text,
        }
        .notification();
        if let Err(e) = n.and_then(|n| cx.send_notification(n)) {
            tracing::warn!(error = %e, "acp peer_message failed");
        }
    }
}

impl Message<ApprovalRequested> for Relay {
    type Reply = ();
    async fn handle(&mut self, a: ApprovalRequested, _: &mut Context<Self, ()>) {
        let Some(cx) = self.connection(&a.session).cloned() else {
            return;
        };
        let api = self.api.clone();
        let tool_call = ToolCallUpdate::new(
            a.id.0.to_string(),
            ToolCallUpdateFields::new()
                .title(a.tool.clone())
                .kind(acp::kind(&a.tool, a.risk))
                .raw_input(a.args),
        );
        let req = RequestPermissionRequest::new(
            AcpSessionId::new(a.session.to_string()),
            tool_call,
            vec![
                PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
            ],
        );
        let id = a.id;
        // A plain tokio task: a slow human must not hold the dispatch loop,
        // and `Resolve` on an already timed-out id is a harmless `false`.
        tokio::spawn(async move {
            let decision = match cx.send_request(req).block_task().await {
                Ok(r) => match r.outcome {
                    RequestPermissionOutcome::Selected(s) if &*s.option_id.0 == "allow" => {
                        Decision::Allow
                    }
                    _ => Decision::Deny,
                },
                Err(e) => {
                    tracing::warn!(error = %e, "acp request_permission failed");
                    Decision::Deny
                }
            };
            let _ = api.resolve_approval(id, decision).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::{
        BlobResourceContents, EmbeddedResource, ImageContent, TextContent,
    };

    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn describe_names_a_vm_engine() {
        let p = mesh::Peer {
            id: mesh::EngineId("id1".into()),
            name: "lab".into(),
            addrs: vec![],
            reachable: true,
            last_seen_ms: 0,
            hub_pref: mesh::HubPref::Never,
            path: mesh::PeerPath::Offline,
            rtt_ms: None,
        };
        let s = mesh::EngineStats {
            id: p.id.clone(),
            uptime_since_ms: 0,
            class: mesh::EngineClass::Vm,
            availability_30d: 1.0,
            harnesses: vec!["native".into(), "omp".into()],
            repos: vec![],
            display: true,
            gpu: false,
            accelerator: None,
            os: String::new(),
            os_name: String::new(),
            arch: String::new(),
            host: String::new(),
        };
        assert_eq!(describe(&p, Some(&s)), "mesh · vm · harnesses: native, omp");
        assert_eq!(describe(&p, None), "mesh");
    }

    /// 67 §8: `_meta.theuth` picks the root's model/effort and installs its
    /// `tools` ceiling; no `_meta` is the plain create; a bad entry is refused.
    #[test]
    fn session_new_meta_becomes_create_options() {
        let plain = NewSessionRequest::new("/work");
        let o = new_session_options(&plain).unwrap();
        assert_eq!(o.cwd.as_deref(), Some("/work"));
        assert_eq!((o.model, o.effort, o.cap), (None, None, None));

        let meta = acp::SessionMeta {
            model: Some("anthropic/claude-opus-4-8".into()),
            effort: Some("high".into()),
            tools: Some(Risk::ReadOnly),
            parent: Some("p".into()),
            parent_engine: Some("e".into()),
            harness: None,
        }
        .to_meta();
        let o = new_session_options(&NewSessionRequest::new("/work").meta(meta)).unwrap();
        assert_eq!(o.model.as_deref(), Some("anthropic/claude-opus-4-8"));
        assert_eq!(o.effort.as_deref(), Some("high"));
        assert_eq!(o.cap, Some(Risk::ReadOnly));

        // `agent: "omp@<this node>"`: the node runs its own `omp` for this root.
        let remote = acp::SessionMeta {
            harness: Some("omp".into()),
            model: Some("anthropic/claude-opus-4-8".into()),
            ..Default::default()
        }
        .to_meta();
        let o = new_session_options(&NewSessionRequest::new("/work").meta(remote)).unwrap();
        assert_eq!(o.harness.as_deref(), Some("omp"));
        assert_eq!(o.model.as_deref(), Some("anthropic/claude-opus-4-8"));
        assert_eq!(o.cap, None);

        // `destructive` is "no ceiling", as for a local subagent.
        let loose = acp::SessionMeta {
            tools: Some(Risk::Destructive),
            ..Default::default()
        }
        .to_meta();
        assert_eq!(
            new_session_options(&NewSessionRequest::new("/work").meta(loose))
                .unwrap()
                .cap,
            None
        );

        let bad: agent_client_protocol::schema::v1::Meta =
            serde_json::from_str(r#"{"theuth":{"tools":"all"}}"#).unwrap();
        let e = new_session_options(&NewSessionRequest::new("/work").meta(bad)).unwrap_err();
        assert_eq!(e.code, agent_client_protocol::ErrorCode::InvalidParams);
    }

    /// 51 §8.3: cost goes out only when one currency covers everything.
    #[test]
    fn acp_cost_only_for_complete_single_currency() {
        use agent::cost::{CostBucket, CostScope, Money};
        let usd = Money::from_f64(0.35, "USD").unwrap();
        let mut c = SessionCost {
            session: SessionId::new(),
            revision: 1,
            history_coverage: Coverage::Complete,
            scope: CostScope::Native,
            direct: CostBucket::default(),
            descendants: CostBucket::default(),
            total: CostBucket {
                returned: vec![usd.clone()],
                ..Default::default()
            },
            children: Vec::new(),
            remote_meter_gaps: 0,
            delta_suspected: false,
            remote_tokens: Vec::new(),
        };
        let cost = acp_cost(&c).unwrap();
        assert_eq!((cost.amount, cost.currency.as_str()), (0.35, "USD"));

        c.total.unavailable = 1;
        assert!(acp_cost(&c).is_none());
        c.total.unavailable = 0;
        c.remote_meter_gaps = 1;
        assert!(acp_cost(&c).is_none());
        c.remote_meter_gaps = 0;
        c.history_coverage = Coverage::Unknown;
        assert!(acp_cost(&c).is_none());
        c.history_coverage = Coverage::Complete;
        c.total.returned.push(Money::from_f64(1.0, "EUR").unwrap());
        assert!(acp_cost(&c).is_none());
        c.total.returned.clear();
        assert!(acp_cost(&c).is_none());
    }

    #[test]
    fn plan_keeps_blocked_and_dropped_honest() {
        let todo = |text: &str, status, reason: Option<&str>| Todo {
            text: text.into(),
            status,
            reason: reason.map(str::to_owned),
        };
        let p = plan(&[
            todo("read", Status::Done, None),
            todo("edit", Status::InProgress, None),
            todo("ship", Status::Blocked, Some("needs creds")),
            todo("gold-plate", Status::Dropped, None),
            todo("test", Status::Pending, None),
        ]);
        let got: Vec<(&str, &PlanEntryStatus)> = p
            .entries
            .iter()
            .map(|e| (e.content.as_str(), &e.status))
            .collect();
        assert_eq!(
            got,
            [
                ("read", &PlanEntryStatus::Completed),
                ("edit", &PlanEntryStatus::InProgress),
                ("ship (blocked: needs creds)", &PlanEntryStatus::Pending),
                ("gold-plate (dropped)", &PlanEntryStatus::Completed),
                ("test", &PlanEntryStatus::Pending),
            ]
        );
    }

    #[test]
    fn image_block_becomes_sha_named_file() {
        let dir = std::env::temp_dir().join(format!("theuth-acp-{}", uuid::Uuid::new_v4()));
        let bytes = b"\x89PNG\r\n\x1a\nnot really a png";
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let pdf = b"%PDF-1.7 not really";
        let pdf_b64 = base64::engine::general_purpose::STANDARD.encode(pdf);
        let blob = |b64: &str, mime: &str, uri: &str| {
            let mut r = BlobResourceContents::new(b64, uri);
            r.mime_type = Some(mime.to_owned());
            ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::BlobResourceContents(r),
            ))
        };
        let blocks = [
            ContentBlock::Text(TextContent::new("look")),
            ContentBlock::Image(ImageContent::new(b64, "image/png")),
            ContentBlock::Image(ImageContent::new("AAAA", "image/tiff")),
            ContentBlock::Image(ImageContent::new("not base64!", "image/png")),
            blob(&pdf_b64, "application/pdf", "file:///r.pdf"),
            blob("AAAA", "application/zip", "file:///r.zip"),
        ];
        let (text, files) = prompt_parts(&blocks, &dir);
        let hex = hex::encode(Sha256::digest(bytes));
        let pdf_hex = hex::encode(Sha256::digest(pdf));
        assert_eq!(
            files,
            [
                dir.join("attachments").join(format!("{}.png", &hex[..16])),
                dir.join("attachments")
                    .join(format!("{}.pdf", &pdf_hex[..16])),
            ]
        );
        assert_eq!(std::fs::read(&files[0]).unwrap(), bytes);
        assert_eq!(std::fs::read(&files[1]).unwrap(), pdf);
        assert!(
            text.starts_with("look\n[image: unsupported image/tiff]\n[image: "),
            "{text}"
        );
        assert!(
            text.ends_with("\n[binary resource file:///r.zip]"),
            "{text}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 67 §4.2: two mesh endpoints on loopback (relay off, no discovery); the
    /// child side accepts an `ALPN_ACP` stream and speaks ACP over
    /// [`acp::line_duplex`] exactly as [`serve_mesh`] does; the parent side
    /// dials with an [`acp::MeshDialer`] through [`acp::Harness::connect`],
    /// opens a session with `_meta.theuth` and receives one `session/update`.
    /// Only the byte stream differs from a link conn, so the ACP agent here
    /// is a two-method stand-in for [`serve`] (a full engine is not needed to
    /// prove the transport).
    #[tokio::test]
    async fn child_session_over_the_mesh_loopback() {
        use agent_client_protocol::schema::v1::NewSessionResponse;
        use replica::{PeerId, Transport};
        use std::time::Duration;

        async fn engine(name: &str) -> (Arc<mesh::net::Net>, PathBuf) {
            let dir = std::env::temp_dir().join(format!(
                "theuth-acp-mesh-{}-{}",
                name,
                uuid::Uuid::now_v7()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let identity = mesh::identity::Identity::load_or_create(&dir).unwrap();
            let store = mesh::store::Store::open(
                &dir.join("mesh.sqlite"),
                mesh::EngineId(identity.secret.public().to_string()),
            )
            .await
            .unwrap();
            let net = mesh::net::Net::bind(
                mesh::net::BindOpts {
                    secret: identity.secret,
                    relay: String::new(),
                    discovery: false,
                    bind_addr: Some("127.0.0.1:0".parse().unwrap()),
                    storm_frames_per_sec: 50,
                    first_seen_ms: identity.first_seen_ms,
                    vault: None,
                    mdns: None,
                },
                store,
                name.into(),
                mesh::HubPref::Eligible,
            )
            .await
            .unwrap();
            (net, dir)
        }
        let (parent, dir_p) = engine("parent").await;
        let (child, dir_c) = engine("child").await;
        let mut events = parent.subscribe_peers();
        parent
            .add_peer(&child.pairing_string().await)
            .await
            .unwrap();
        let connected = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            connected,
            replica::PeerEvent::Connected(PeerId(child.id.0.clone()))
        );

        // The child engine: one accepted stream, one ACP agent on it.
        let seen_meta = Arc::new(parking_lot::Mutex::new(None));
        let (c, meta_slot) = (child.clone(), seen_meta.clone());
        let server = tokio::spawn(async move {
            let (from, stream) = c.accept(replica::ALPN_ACP).await.unwrap();
            let d = acp::line_duplex(stream);
            Agent
                .builder()
                .on_receive_request(
                    async move |req: InitializeRequest, responder, _cx| {
                        responder.respond(InitializeResponse::new(req.protocol_version))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: NewSessionRequest, responder, cx| {
                        *meta_slot.lock() = acp::SessionMeta::from_meta(req.meta.as_ref())
                            .ok()
                            .flatten();
                        let id = AcpSessionId::new("child-1");
                        cx.send_notification(SessionNotification::new(
                            id.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                "hello from the child".into(),
                            )),
                        ))?;
                        responder.respond(NewSessionResponse::new(id))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(
                    Lines::new(d.outgoing, d.incoming),
                    async move |cx: ConnectionTo<Client>| {
                        cx.incoming_closed().await;
                        Ok(())
                    },
                )
                .await
                .unwrap();
            from
        });

        // The parent engine: `spawn_subagent{agent:"child"}` as the supervisor does it.
        let dialer = acp::MeshDialer {
            name: "child".into(),
            peer: PeerId(child.id.0.clone()),
            self_id: parent.id.0.clone(),
            transport: parent.clone(),
        };
        assert_eq!(acp::Dialer::engine_id(&dialer), Some(child.id.0.as_str()));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let harness = acp::Harness::connect(
            acp::Connect {
                transport: acp::Transport::Dial(Arc::new(dialer)),
                cwd: dir_p.clone(),
                resume: None,
                stderr_log: dir_p.join("acp.log"),
                meta: Some(acp::SessionMeta {
                    tools: Some(Risk::ReadOnly),
                    parent: Some("parent-session".into()),
                    ..Default::default()
                }),
                model: None,
                effort: None,
                mcp_servers: Vec::new(),
            },
            tx,
        )
        .await
        .unwrap();
        let update = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match update {
            acp::Update::Session {
                session,
                update: SessionUpdate::AgentMessageChunk(c),
            } => {
                assert_eq!(
                    session,
                    harness.session_id(),
                    "141 §7.1: the wire sessionId rides along"
                );
                assert_eq!(acp::content_block_text(&c.content), "hello from the child");
            }
            other => panic!("expected a message chunk, got {other:?}"),
        }
        harness.close();
        let from = tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(from, PeerId(parent.id.0.clone()));
        let meta = seen_meta
            .lock()
            .take()
            .expect("_meta.theuth reached the child");
        assert_eq!(
            (meta.tools, meta.parent.as_deref()),
            (Some(Risk::ReadOnly), Some("parent-session"))
        );

        parent.shutdown().await;
        child.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir_p);
        let _ = std::fs::remove_dir_all(&dir_c);
    }
}
