//! Client side (design doc 50 §4, §6.3, §7, §9): one [`PeerClient`] per
//! configured outbound peer, registered with the agent as its
//! [`PeerSender`]. The SDK resolves interfaces, frames JSON-RPC and parses
//! SSE; this file owns the egress policy (validated origin, pinned DNS, no
//! redirects, bearer only in the header), the 64 KiB card bound, outcome
//! classification (never a blind resend) and following a remote task back to
//! the agent through `RemoteTaskUpdate`.
//!
//! Raw bounds (§8) sit in the vendored `a2a-client`: one SSE frame is capped
//! at `a2a_client::MAX_SSE_EVENT_BYTES` (64 KiB) before buffering and ends the
//! stream with one error frame (the follower treats that as a stream end and
//! falls back to `GetTask` polling); one unary response body is capped at
//! `a2a_client::MAX_RESPONSE_BYTES` (2 MiB) before JSON decoding. The bounds
//! here apply after decoding, to what is retained across events.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use a2a_client::jsonrpc::JsonRpcTransportFactory;
use a2a_client::middleware::CallInterceptor;
use a2a_client::{A2AClient, A2AClientFactory, Transport};
use a2a_types::*;
use agent::Agents;
use agent::communication::network::{
    OutboundPeerMessage, PeerSender, RemoteReply, RemoteSendError, RemoteTaskUpdate, RemoteUpdate,
};
use async_trait::async_trait;
use futures::StreamExt;
use kameo::actor::ActorRef;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use secrets::Vault;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::config::{CARD_SUFFIX, Peer, Token, egress_allowed};

/// Fetched Agent Card bound (§8); rejected before parsing more.
pub const CARD_LIMIT: usize = 64 * 1024;
/// TCP connect deadline (§8).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Unary admission request deadline (§8). Expiry is an *unknown* outcome.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Stream idle deadline (§8).
pub const STREAM_IDLE: Duration = Duration::from_secs(120);
/// Cumulative text artifact bytes accepted per remote task (§8).
pub const ARTIFACT_BYTES_MAX: usize = 1 << 20;
/// Serialized JSON bytes of one retained task snapshot, whatever the field
/// (metadata, names, history, empty parts): what the SDK lets one response
/// carry is also what a stream may accumulate.
pub const TASK_BYTES_MAX: usize = a2a_client::MAX_RESPONSE_BYTES;
/// Artifacts, artifact parts and history messages retained per task.
pub const TASK_ITEMS_MAX: usize = 4096;
/// Status message text bound (§8).
pub const STATUS_TEXT_MAX: usize = 32 * 1024;
/// Remote free text that reaches a log, status or error: card name, error
/// message. Redacted, then cut here.
pub const REMOTE_TEXT_MAX: usize = 4 * 1024;
/// How long a remote task is followed before the outcome is reported unknown.
/// ponytail: one flat ceiling; make it per-peer config if a real peer needs longer.
pub const FOLLOW_MAX: Duration = Duration::from_secs(6 * 3600);
const POLL_MIN: Duration = Duration::from_secs(2);
const POLL_MAX: Duration = Duration::from_secs(30);

/// 183 review 1, 2: the follower's deadline at `now`. A contact's owner may take up to the
/// hold TTL to answer a first message; while the task waits for that (`waiting`:
/// `AUTH_REQUIRED`) the deadline slides to `now + FOLLOW_MAX`, never past `started +
/// HOLD_TTL + FOLLOW_MAX`, so the answer after a late Allow still reaches the sender.
fn follow_deadline(
    started: tokio::time::Instant,
    deadline: tokio::time::Instant,
    now: tokio::time::Instant,
    waiting: bool,
) -> tokio::time::Instant {
    if !waiting {
        return deadline;
    }
    deadline
        .max(now + FOLLOW_MAX)
        .min(started + agent::communication::network::HOLD_TTL + FOLLOW_MAX)
}

type Client = A2AClient<Box<dyn Transport>>;

/// A resolved card and its negotiated client.
pub struct Connected {
    client: Client,
    /// Card `name`, for status.
    pub card_name: String,
}

/// Why a peer could not be connected.
#[derive(Debug)]
pub enum ConnectError {
    /// Deterministic: policy, credentials or an incompatible card. Reload
    /// validation fails on it; sends are refused.
    Incompatible(String),
    /// Network: retried on the next send; a reload records it per peer.
    Transport(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Incompatible(s) | Self::Transport(s) => f.write_str(s),
        }
    }
}

/// The peer credential travels only as an interceptor-set `Authorization`
/// header; never in a message, card, model argument or log.
struct Bearer(Token);

#[async_trait]
impl CallInterceptor for Bearer {
    async fn before(
        &self,
        _method: &str,
        params: &mut a2a_client::ServiceParams,
    ) -> Result<(), A2AError> {
        params.insert(
            AUTHORIZATION.as_str().to_owned(),
            vec![format!("Bearer {}", self.0.as_str())],
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Egress: resolve, pin, fetch the card, pick the interface
// ---------------------------------------------------------------------------

/// Resolve the host through the egress policy (§9). Literal IPs go through
/// the same filter. The approved addresses are pinned on every request.
async fn pinned_addrs(
    base: &Url,
    allow_private: bool,
) -> Result<(String, Vec<SocketAddr>), ConnectError> {
    let host = base
        .host_str()
        .ok_or_else(|| ConnectError::Incompatible("card url has no host".into()))?
        .to_owned();
    let port = base
        .port_or_known_default()
        .ok_or_else(|| ConnectError::Incompatible("card url has no port".into()))?;
    let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| ConnectError::Transport(format!("dns {host}: {e}")))?
        .collect();
    let allowed: Vec<SocketAddr> = resolved
        .iter()
        .copied()
        .filter(|a| egress_allowed(a.ip(), allow_private))
        .collect();
    if allowed.is_empty() {
        return Err(ConnectError::Incompatible(format!(
            "{host} resolves only to addresses the egress policy refuses ({} candidate(s)); set allow_private for loopback/private peers",
            resolved.len()
        )));
    }
    Ok((host, allowed))
}

/// Pinned, redirect-free, proxy-free HTTP client. Any proxy (environment or
/// system, which the SDK's reqwest features may enable) would resolve the host
/// itself and bypass the pinned addresses, so none is consulted (§9). With
/// `card` the credential is a default header and the whole request has a
/// deadline (`AgentCardResolver` has no interceptor hook and no size bound, so
/// the card is fetched here); without it the client serves the SDK transport:
/// unary calls get their deadline per call, streams only idle out.
fn http(
    base: &Url,
    host: &str,
    addrs: &[SocketAddr],
    card: Option<&Token>,
) -> Result<reqwest::Client, ConnectError> {
    let mut b = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .resolve_to_addrs(host, addrs)
        .https_only(base.scheme() == "https")
        .user_agent(concat!("theuth-a2a/", env!("CARGO_PKG_VERSION")));
    match card {
        Some(token) => {
            let mut h = HeaderMap::new();
            let v = HeaderValue::from_str(&format!("Bearer {}", token.as_str())).map_err(|_| {
                ConnectError::Incompatible("outbound secret is not a valid header value".into())
            })?;
            h.insert(AUTHORIZATION, v);
            b = b.default_headers(h).timeout(REQUEST_TIMEOUT);
        }
        None => b = b.read_timeout(STREAM_IDLE),
    }
    b.build()
        .map_err(|e| ConnectError::Transport(format!("http client: {e}")))
}

/// `{base}/.well-known/agent-card.json`, at most [`CARD_LIMIT`] bytes, as the SDK type.
async fn fetch_card(http: &reqwest::Client, base: &Url) -> Result<AgentCard, ConnectError> {
    let url = format!("{}{CARD_SUFFIX}", base.as_str().trim_end_matches('/'));
    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| ConnectError::Transport(format!("card fetch: {e}")))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ConnectError::Incompatible(format!(
            "card fetch: HTTP {status}; outbound credential refused"
        )));
    }
    if !status.is_success() {
        return Err(ConnectError::Transport(format!(
            "card fetch: HTTP {status}"
        )));
    }
    if resp.content_length().is_some_and(|n| n > CARD_LIMIT as u64) {
        return Err(ConnectError::Incompatible("card larger than 64 KiB".into()));
    }
    let mut body = Vec::new();
    let mut chunks = resp.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| ConnectError::Transport(format!("card body: {e}")))?;
        if body.len() + chunk.len() > CARD_LIMIT {
            return Err(ConnectError::Incompatible("card larger than 64 KiB".into()));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<AgentCard>(&body)
        .map_err(|e| ConnectError::Incompatible(format!("card is not an AgentCard: {e}")))
}

/// The card's first `JSONRPC` interface at exactly protocol version `1.0`
/// (§3.3: no other version is served or consumed), which must sit on the
/// configured origin (§9: no endpoint substitution, no credentials in the URL).
fn compatible_interface(base: &Url, card: &AgentCard) -> Result<AgentInterface, ConnectError> {
    let iface = card
        .supported_interfaces
        .iter()
        .find(|i| i.protocol_binding == TRANSPORT_PROTOCOL_JSONRPC && i.protocol_version == VERSION)
        .cloned()
        .ok_or_else(|| {
            ConnectError::Incompatible(format!(
                "card advertises no JSONRPC/{VERSION} interface ({} interface(s))",
                card.supported_interfaces.len()
            ))
        })?;
    // The card came back over a request carrying the bearer, so nothing the
    // card says is echoed raw: not the unparsable string, not a URL whose
    // userinfo could be that bearer. Only the origin, which has neither.
    let url = Url::parse(&iface.url)
        .map_err(|e| ConnectError::Incompatible(format!("interface url does not parse: {e}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConnectError::Incompatible(
            "interface url carries credentials".into(),
        ));
    }
    let same_origin = url.scheme() == base.scheme()
        && url.host_str() == base.host_str()
        && url.port_or_known_default() == base.port_or_known_default();
    if !same_origin {
        return Err(ConnectError::Incompatible(format!(
            "interface url is not on the configured origin {}",
            base.origin().ascii_serialization()
        )));
    }
    Ok(iface)
}

/// Reload-time validation of an outbound peer (§6.1): egress policy, card
/// fetch and interface compatibility, without any identity commit. Returns the
/// card name. `Incompatible` fails the reload; `Transport` is a peer status.
/// Both the name and any error are reload log/status text, so the bearer the
/// peer just received is scrubbed from them whatever path produced them.
pub async fn probe(peer: &Peer) -> Result<String, ConnectError> {
    let (Some(base), Some(token)) = (&peer.card, &peer.outbound) else {
        return Err(ConnectError::Incompatible(format!(
            "peer {} is inbound-only",
            peer.name
        )));
    };
    let scrub = |s: &str| {
        let s = s.replace(token.as_str(), "[redacted outbound secret]");
        cut_remote(s)
    };
    let run = async {
        let (host, addrs) = pinned_addrs(base, peer.allow_private).await?;
        let card = fetch_card(&http(base, &host, &addrs, Some(token))?, base).await?;
        compatible_interface(base, &card)?;
        Ok::<_, ConnectError>(card.name)
    };
    match run.await {
        Ok(name) => Ok(scrub(&name)),
        Err(ConnectError::Incompatible(e)) => Err(ConnectError::Incompatible(scrub(&e))),
        Err(ConnectError::Transport(e)) => Err(ConnectError::Transport(scrub(&e))),
    }
}

/// Cut remote free text at [`REMOTE_TEXT_MAX`] on a char boundary.
fn cut_remote(mut s: String) -> String {
    if s.len() > REMOTE_TEXT_MAX {
        let mut n = REMOTE_TEXT_MAX;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
        s.push('…');
    }
    s
}

// ---------------------------------------------------------------------------
// PeerClient
// ---------------------------------------------------------------------------

/// The outbound exchange a follower reports for: its route binding and the
/// local message id (`OutboundPeerMessage::record.receipt.id`).
#[derive(Clone, Copy, Debug)]
struct Exchange {
    binding: Uuid,
    id: Uuid,
}

/// How a [`PeerClient`] reaches its peer.
enum Link {
    /// Design 50: an HTTPS card origin and the bearer we present (the only copy of it).
    Http {
        base: Url,
        token: Token,
        allow_private: bool,
    },
    /// 183 §7.2: a contact over `theuth/a2a/1`, authenticated by its pinned endpoint id.
    Iroh {
        net: Arc<mesh::net::Net>,
        eid: String,
        display: String,
    },
}

/// One configured outbound peer (§6.1). Holds the only copy of its bearer.
pub struct PeerClient {
    /// Owner alias.
    pub name: String,
    link: Link,
    agents: ActorRef<Agents>,
    vault: Option<Arc<Vault>>,
    /// Cancelled on reload/removal/disable: in-flight sends and followers stop.
    generation: CancellationToken,
    connected: Mutex<Option<Arc<Connected>>>,
    /// Remote task ids with a live follower: one per task, whatever the
    /// number of sends (replies) on it.
    following: parking_lot::Mutex<HashSet<String>>,
    /// Card name from the reload probe, before the first send connects.
    probed_name: parking_lot::Mutex<Option<String>>,
}

impl PeerClient {
    /// New client for a validated outbound peer; nothing is fetched until
    /// [`Self::connect`]. `Err` when the peer has no outbound side.
    pub fn new(
        peer: &Peer,
        agents: ActorRef<Agents>,
        vault: Option<Arc<Vault>>,
        generation: CancellationToken,
    ) -> Result<Self, String> {
        let (Some(base), Some(token)) = (peer.card.clone(), peer.outbound.clone()) else {
            return Err(format!("peer {} is inbound-only", peer.name));
        };
        Ok(Self::with(
            peer,
            Link::Http {
                base,
                token,
                allow_private: peer.allow_private,
            },
            agents,
            vault,
            generation,
        ))
    }

    /// 183 §7.7: the client of a contact (`peer.eid` set), over `net`.
    pub fn contact(
        peer: &Peer,
        display: &str,
        net: Arc<mesh::net::Net>,
        agents: ActorRef<Agents>,
        vault: Option<Arc<Vault>>,
        generation: CancellationToken,
    ) -> Result<Self, String> {
        let eid = peer
            .eid
            .clone()
            .ok_or_else(|| format!("peer {} is not a contact", peer.name))?;
        Ok(Self::with(
            peer,
            Link::Iroh {
                net,
                eid,
                display: display.to_owned(),
            },
            agents,
            vault,
            generation,
        ))
    }

    fn with(
        peer: &Peer,
        link: Link,
        agents: ActorRef<Agents>,
        vault: Option<Arc<Vault>>,
        generation: CancellationToken,
    ) -> Self {
        Self {
            name: peer.name.clone(),
            link,
            agents,
            vault,
            generation,
            connected: Mutex::new(None),
            following: parking_lot::Mutex::new(HashSet::new()),
            probed_name: parking_lot::Mutex::new(None),
        }
    }

    /// Same identity, origin, credential and egress policy: a reload keeps
    /// this client (and its followers) instead of revoking it.
    pub fn matches(&self, peer: &Peer) -> bool {
        match &self.link {
            Link::Http {
                base,
                token,
                allow_private,
            } => {
                peer.card.as_ref() == Some(base)
                    && peer.outbound.as_ref() == Some(token)
                    && peer.allow_private == *allow_private
            }
            Link::Iroh { eid, .. } => peer.eid.as_ref() == Some(eid),
        }
    }

    /// The route's revocation token (handed to `RegisterPeerSender`).
    pub fn generation(&self) -> &CancellationToken {
        &self.generation
    }

    /// Record the card name the reload probe saw, redacted and cut.
    pub fn set_probed_name(&self, name: &str) {
        *self.probed_name.lock() = Some(self.remote_text(name));
    }

    /// The card name: from the live client, else from the reload probe.
    pub async fn card_name(&self) -> Option<String> {
        if let Some(c) = &*self.connected.lock().await {
            return Some(c.card_name.clone());
        }
        self.probed_name.lock().clone()
    }

    /// Vault-known secrets, then this peer's own bearer (the vault may not
    /// hold it, or not be installed), are scrubbed from remote text.
    fn redact(&self, text: &str) -> String {
        let text = match &self.vault {
            Some(v) => v.redact(text),
            None => std::borrow::Cow::Borrowed(text),
        };
        match &self.link {
            Link::Http { token, .. } if text.contains(token.as_str()) => {
                text.replace(token.as_str(), "[redacted outbound secret]")
            }
            _ => text.into_owned(),
        }
    }

    /// Remote free text bound for logs, status and errors: redacted, then cut
    /// at [`REMOTE_TEXT_MAX`] on a char boundary.
    fn remote_text(&self, text: &str) -> String {
        cut_remote(self.redact(text))
    }

    fn redact_message(&self, m: &mut Message) {
        for p in &mut m.parts {
            if let PartContent::Text(t) = &mut p.content {
                *t = self.redact(t);
            }
        }
    }

    fn redact_task(&self, t: &mut Task) {
        if let Some(m) = t.status.message.as_mut() {
            self.redact_message(m);
        }
        for a in t.artifacts.iter_mut().flatten() {
            for p in &mut a.parts {
                if let PartContent::Text(s) = &mut p.content {
                    *s = self.redact(s);
                }
            }
        }
        for m in t.history.iter_mut().flatten() {
            self.redact_message(m);
        }
    }

    /// Resolve the card and negotiate the client once per generation (§4). 183 §7.7: a
    /// contact's client is built per send (each call opens its own stream, nothing to
    /// negotiate), aimed at `export` (`peer:<contact>/<export>`) or the contact's first.
    pub async fn connect(&self, export: Option<&str>) -> Result<Arc<Connected>, ConnectError> {
        let (base, token, allow_private) = match &self.link {
            Link::Http {
                base,
                token,
                allow_private,
            } => (base, token, *allow_private),
            // The card was verified at the ceremony; the channel is the pin.
            Link::Iroh { net, eid, display } => {
                let transport: Box<dyn Transport> =
                    Box::new(crate::contacts::reach::IrohTransport {
                        net: net.clone(),
                        eid: eid.clone(),
                        hints: Default::default(),
                        export: export.map(str::to_owned),
                    });
                return Ok(Arc::new(Connected {
                    client: a2a_client::A2AClient::new(transport),
                    card_name: self.remote_text(display),
                }));
            }
        };
        let mut slot = self.connected.lock().await;
        if let Some(c) = &*slot {
            return Ok(c.clone());
        }
        let (host, addrs) = pinned_addrs(base, allow_private).await?;
        let card = fetch_card(&http(base, &host, &addrs, Some(token))?, base).await?;
        let wanted = compatible_interface(base, &card)?;
        let transport = JsonRpcTransportFactory::new(Some(http(base, &host, &addrs, None)?));
        let factory = A2AClientFactory::builder()
            .no_defaults()
            .register(Arc::new(transport))
            .with_interceptor(Arc::new(Bearer(token.clone())))
            .build();
        let (client, selected) = factory
            .create_from_card_with_interface(&card)
            .await
            .map_err(|e| {
                ConnectError::Incompatible(format!("client negotiation: {}", e.message))
            })?;
        if selected.url != wanted.url || selected.protocol_binding != TRANSPORT_PROTOCOL_JSONRPC {
            return Err(ConnectError::Incompatible(
                "factory selected a different interface than the validated one".into(),
            ));
        }
        let c = Arc::new(Connected {
            client,
            card_name: self.remote_text(&card.name),
        });
        *slot = Some(c.clone());
        Ok(c)
    }

    async fn report(&self, ex: Exchange, update: RemoteUpdate) {
        match self
            .agents
            .ask(RemoteTaskUpdate {
                binding: ex.binding,
                exchange: ex.id,
                update,
            })
            .await
        {
            Ok(()) => {}
            Err(kameo::error::SendError::HandlerError(e)) => {
                tracing::debug!(peer = %self.name, exchange = %ex.id, "remote update refused: {e}");
            }
            Err(e) => {
                tracing::warn!(peer = %self.name, exchange = %ex.id, "agent unavailable: {e}")
            }
        }
    }

    /// Start following `task` for `exchange` unless a follower for that task
    /// already runs (a reply on the same task reuses it).
    fn follow_once(
        self: &Arc<Self>,
        connected: Arc<Connected>,
        task: Task,
        kept: Retained,
        exchange: Exchange,
    ) {
        if !self.following.lock().insert(task.id.clone()) {
            return;
        }
        tokio::spawn(self.clone().follow(connected, task, kept, exchange));
    }

    /// Keep watching a remote task until it is terminal (§6.3, §7): subscribe,
    /// fall back to bounded polling, report each changed snapshot (including
    /// `INPUT_REQUIRED`/`AUTH_REQUIRED` and any out-of-band resume after
    /// them). Revocation, the follow ceiling or a vanished task end with an
    /// `Unknown` outcome; a bound violation is a deliberate `Failed`. Never
    /// resends.
    async fn follow(
        self: Arc<Self>,
        connected: Arc<Connected>,
        mut task: Task,
        mut kept: Retained,
        exchange: Exchange,
    ) {
        struct Done<'a>(&'a PeerClient, String);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.following.lock().remove(&self.1);
            }
        }
        let _done = Done(&self, task.id.clone());

        let started = tokio::time::Instant::now();
        let mut deadline = started + FOLLOW_MAX;
        let mut seen = fingerprint(&task);
        let mut backoff = POLL_MIN;
        loop {
            deadline = follow_deadline(
                started,
                deadline,
                tokio::time::Instant::now(),
                task.status.state == TaskState::AuthRequired,
            );
            if self.generation.is_cancelled() {
                self.report(
                    exchange,
                    RemoteUpdate::Unknown("route revoked while following the remote task".into()),
                )
                .await;
                return;
            }
            if tokio::time::Instant::now() > deadline {
                self.report(
                    exchange,
                    RemoteUpdate::Unknown(format!(
                        "remote task not terminal after {}s",
                        (deadline - started).as_secs()
                    )),
                )
                .await;
                return;
            }
            let sub = SubscribeToTaskRequest {
                id: task.id.clone(),
                tenant: None,
            };
            let stream = tokio::select! {
                () = self.generation.cancelled() => continue,
                r = tokio::time::timeout(REQUEST_TIMEOUT, connected.client.subscribe_to_task(&sub)) => r,
            };
            match stream {
                Ok(Ok(mut events)) => {
                    backoff = POLL_MIN;
                    loop {
                        // The ceiling holds inside an active stream too: a
                        // peer that never stops sending must not hold the
                        // follower forever.
                        let next = tokio::select! {
                            () = self.generation.cancelled() => break,
                            () = tokio::time::sleep_until(deadline) => break,
                            n = tokio::time::timeout(STREAM_IDLE, events.next()) => n,
                        };
                        match next {
                            Ok(Some(Ok(ev))) => {
                                if let Err(e) = apply(&mut task, ev, &mut kept) {
                                    self.report(exchange, RemoteUpdate::Failed(e)).await;
                                    return;
                                }
                                if self.publish(exchange, &task, &mut seen).await {
                                    return;
                                }
                            }
                            Ok(Some(Err(e))) => {
                                tracing::debug!(peer = %self.name, exchange = %exchange.id, "stream error: {}", self.remote_text(&e.message));
                                break;
                            }
                            Ok(None) | Err(_) => break,
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::debug!(peer = %self.name, exchange = %exchange.id, "subscribe refused: {}", self.remote_text(&e.message))
                }
                Err(_) => {
                    tracing::debug!(peer = %self.name, exchange = %exchange.id, "subscribe timed out")
                }
            }
            // Poll: the SDK's GetTask, backing off, until a terminal snapshot.
            tokio::select! {
                () = self.generation.cancelled() => continue,
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(POLL_MAX);
            let get = GetTaskRequest {
                id: task.id.clone(),
                history_length: Some(0),
                tenant: None,
            };
            match tokio::time::timeout(REQUEST_TIMEOUT, connected.client.get_task(&get)).await {
                Ok(Ok(fresh)) => {
                    kept = match Retained::of(&fresh) {
                        Ok(k) => k,
                        Err(e) => {
                            self.report(exchange, RemoteUpdate::Failed(e)).await;
                            return;
                        }
                    };
                    task = fresh;
                    if self.publish(exchange, &task, &mut seen).await {
                        return;
                    }
                }
                Ok(Err(e)) if e.code == error_code::TASK_NOT_FOUND => {
                    self.report(
                        exchange,
                        RemoteUpdate::Unknown("remote task no longer exists".into()),
                    )
                    .await;
                    return;
                }
                Ok(Err(e)) => {
                    tracing::debug!(peer = %self.name, exchange = %exchange.id, "get_task: {}", self.remote_text(&e.message))
                }
                Err(_) => {
                    tracing::debug!(peer = %self.name, exchange = %exchange.id, "get_task timed out")
                }
            }
        }
    }

    /// Report a redacted snapshot when it changed; `true` when terminal.
    async fn publish(&self, exchange: Exchange, task: &Task, seen: &mut String) -> bool {
        let now = fingerprint(task);
        if now != *seen {
            *seen = now;
            let mut snap = task.clone();
            self.redact_task(&mut snap);
            self.report(exchange, RemoteUpdate::Task(snap)).await;
        }
        task.status.state.is_terminal()
    }

    async fn send_outbound(
        self: Arc<Self>,
        outbound: OutboundPeerMessage,
    ) -> Result<RemoteReply, RemoteSendError> {
        if self.generation.is_cancelled() || !outbound.generation.live() {
            return Err(RemoteSendError::Refused(
                "communication route revoked".into(),
            ));
        }
        let connected = self
            .connect(outbound.export.as_deref())
            .await
            .map_err(|e| {
                RemoteSendError::Refused(format!(
                    "peer {}: {}",
                    self.name,
                    self.remote_text(&e.to_string())
                ))
            })?;
        let mut message = Message::new(
            Role::User,
            vec![Part::text(self.redact(&outbound.record.text))],
        );
        message.message_id = outbound.request_id.clone();
        message.task_id = outbound.task_id.clone();
        message.context_id = outbound.context_id.clone();
        // 186 (D5, D9): a full contact share's references and chain count.
        let metadata = outbound.full.as_ref().map(|f| {
            message.reference_task_ids = f.reference_task.clone().map(|t| vec![t]);
            f.metadata()
        });
        message.metadata = metadata;
        let request = SendMessageRequest {
            message,
            configuration: Some(SendMessageConfiguration {
                accepted_output_modes: Some(vec!["text/plain".into()]),
                task_push_notification_config: None,
                history_length: Some(0),
                // §6.2: outbound work is asynchronous; the follower reports the rest.
                return_immediately: Some(true),
            }),
            metadata: None,
            tenant: None,
        };
        if !outbound.generation.live() {
            return Err(RemoteSendError::Refused(
                "communication route revoked".into(),
            ));
        }
        let sent = tokio::select! {
            () = self.generation.cancelled() => {
                return Err(RemoteSendError::Unknown("route revoked while the request was in flight".into()));
            }
            r = tokio::time::timeout(REQUEST_TIMEOUT, connected.client.send_message(&request)) => r,
        };
        match sent {
            Err(_) => Err(RemoteSendError::Unknown(format!(
                "no response from peer {} within {}s; the message may have arrived",
                self.name,
                REQUEST_TIMEOUT.as_secs()
            ))),
            Ok(Err(e)) if refused_before_send(&e) => Err(RemoteSendError::Refused(format!(
                "peer {}: {}",
                self.name,
                self.remote_text(&e.message)
            ))),
            Ok(Err(e)) if e.code == error_code::INTERNAL_ERROR => {
                // SDK-local failure after the request may have left (HTTP,
                // body bound, malformed or missing result) or a remote
                // internal error: the peer may have executed it. Drop the
                // negotiated client: the next send reconnects and re-pins.
                *self.connected.lock().await = None;
                Err(RemoteSendError::Unknown(format!(
                    "peer {}: {}",
                    self.name,
                    self.remote_text(&e.message)
                )))
            }
            Ok(Err(e)) => Err(RemoteSendError::Refused(format!(
                "peer {} refused: {} ({})",
                self.name,
                self.remote_text(&e.message),
                error_reason(e.code)
            ))),
            Ok(Ok(SendMessageResponse::Message(mut m))) => {
                self.redact_message(&mut m);
                Ok(SendMessageResponse::Message(m))
            }
            Ok(Ok(SendMessageResponse::Task(mut task))) => {
                // The peer accepted the message; a snapshot we refuse to hold
                // is not a refusal, and a resend would duplicate it.
                let kept = Retained::of(&task).map_err(|e| {
                    RemoteSendError::Unknown(format!(
                        "peer {} accepted the message but returned a task out of bounds: {e}",
                        self.name
                    ))
                })?;
                self.redact_task(&mut task);
                if !task.status.state.is_terminal() {
                    self.follow_once(
                        connected,
                        task.clone(),
                        kept,
                        Exchange {
                            binding: outbound.binding,
                            id: outbound.record.receipt.id,
                        },
                    );
                }
                Ok(SendMessageResponse::Task(task))
            }
        }
    }
}

/// The `Arc<dyn PeerSender>` the agent registers: a handle to the client, so
/// followers spawned by a send own the client for as long as they run.
pub struct Sender(pub Arc<PeerClient>);

#[async_trait]
impl PeerSender for Sender {
    async fn send(&self, outbound: OutboundPeerMessage) -> Result<RemoteReply, RemoteSendError> {
        self.0.clone().send_outbound(outbound).await
    }
}

// ---------------------------------------------------------------------------
// Snapshot folding and bounds
// ---------------------------------------------------------------------------

fn text_bytes(parts: &[Part]) -> usize {
    parts.iter().filter_map(Part::as_text).map(str::len).sum()
}

fn artifact_text_bytes(task: &Task) -> usize {
    task.artifacts
        .iter()
        .flatten()
        .map(|a| text_bytes(&a.parts))
        .sum()
}

/// Serialized JSON size: the one measure under which metadata, names and
/// empty parts weigh what they cost to keep.
fn json_len<T: Serialize>(v: &T) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

/// Artifacts, their parts, history messages and their parts.
fn items(task: &Task) -> usize {
    let artifacts: usize = task
        .artifacts
        .iter()
        .flatten()
        .map(|a| 1 + a.parts.len())
        .sum();
    let history: usize = task
        .history
        .iter()
        .flatten()
        .map(|m| 1 + m.parts.len())
        .sum();
    artifacts + history
}

/// Running §8 accounting for one retained snapshot, kept incrementally so a
/// stream of small events costs the event, not a re-serialization of the task.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Retained {
    /// Cumulative artifact text bytes.
    text: usize,
    /// Serialized bytes of the whole snapshot (envelope over-counted on
    /// append, never under).
    bytes: usize,
}

impl Retained {
    /// Measure a whole snapshot against every bound.
    fn of(task: &Task) -> Result<Self, String> {
        if task
            .artifacts
            .iter()
            .flatten()
            .any(|a| a.parts.iter().any(|p| p.as_text().is_none()))
        {
            return Err("remote artifact carries a non-text part".into());
        }
        status_text_ok(task.status.message.as_ref())?;
        let kept = Self {
            text: artifact_text_bytes(task),
            bytes: json_len(task),
        };
        kept.check(items(task))?;
        Ok(kept)
    }

    fn check(self, count: usize) -> Result<(), String> {
        if self.text > ARTIFACT_BYTES_MAX {
            return Err("remote artifacts exceed the 1 MiB text bound".into());
        }
        if self.bytes > TASK_BYTES_MAX {
            return Err(format!(
                "remote task exceeds the {TASK_BYTES_MAX}-byte bound"
            ));
        }
        if count > TASK_ITEMS_MAX {
            return Err(format!(
                "remote task exceeds {TASK_ITEMS_MAX} artifacts, parts and history messages"
            ));
        }
        Ok(())
    }
}

fn status_text_ok(m: Option<&Message>) -> Result<(), String> {
    if m.is_some_and(|m| text_bytes(&m.parts) > STATUS_TEXT_MAX) {
        return Err("remote status message exceeds the 32 KiB bound".into());
    }
    Ok(())
}

/// What makes two snapshots "the same notice": state, status timestamp and
/// message id, artifact ids and sizes. Repeated identical snapshots are not
/// re-reported.
fn fingerprint(task: &Task) -> String {
    let mut s = format!(
        "{:?}|{}|{}",
        task.status.state,
        task.status
            .timestamp
            .map(|t| t.timestamp_micros())
            .unwrap_or_default(),
        task.status
            .message
            .as_ref()
            .map_or("", |m| m.message_id.as_str()),
    );
    for a in task.artifacts.iter().flatten() {
        s.push('|');
        s.push_str(&a.artifact_id);
        s.push(':');
        s.push_str(&text_bytes(&a.parts).to_string());
    }
    s
}

/// Fold one stream event into the retained snapshot, enforcing §8 bounds
/// before anything is kept; on `Err` the snapshot is unchanged.
fn apply(task: &mut Task, ev: StreamResponse, kept: &mut Retained) -> Result<(), String> {
    match ev {
        StreamResponse::Task(t) => {
            *kept = Retained::of(&t)?;
            *task = t;
        }
        StreamResponse::StatusUpdate(u) => {
            status_text_ok(u.status.message.as_ref())?;
            let next = Retained {
                text: kept.text,
                bytes: kept
                    .bytes
                    .saturating_sub(json_len(&task.status))
                    .saturating_add(json_len(&u.status)),
            };
            next.check(items(task))?;
            *kept = next;
            task.status = u.status;
            // 186 (D4): a full contact's receipt rides the status event.
            if let Some(m) = u.metadata {
                task.metadata.get_or_insert_with(Default::default).extend(m);
            }
        }
        StreamResponse::ArtifactUpdate(u) => {
            if u.artifact.parts.iter().any(|p| p.as_text().is_none()) {
                return Err("remote artifact carries a non-text part".into());
            }
            let list = task.artifacts.as_deref().unwrap_or_default();
            let existing = list
                .iter()
                .position(|a| a.artifact_id == u.artifact.artifact_id);
            let appending = u.append == Some(true) && existing.is_some();
            let (old_text, old_bytes, old_items) = match existing {
                Some(i) if !appending => (
                    text_bytes(&list[i].parts),
                    json_len(&list[i]),
                    1 + list[i].parts.len(),
                ),
                _ => (0, 0, 0),
            };
            let next = Retained {
                text: kept.text.saturating_sub(old_text) + text_bytes(&u.artifact.parts),
                bytes: kept
                    .bytes
                    .saturating_sub(old_bytes)
                    .saturating_add(json_len(&u.artifact)),
            };
            next.check(items(task) - old_items + usize::from(!appending) + u.artifact.parts.len())?;
            *kept = next;
            let list = task.artifacts.get_or_insert_with(Vec::new);
            match existing {
                Some(i) if appending => list[i].parts.extend(u.artifact.parts),
                Some(i) => list[i] = u.artifact,
                None => list.push(u.artifact),
            }
        }
        StreamResponse::Message(m) => {
            // A message on a task stream is a status-bearing agent message.
            status_text_ok(Some(&m))?;
            let next = Retained {
                text: kept.text,
                bytes: kept
                    .bytes
                    .saturating_sub(json_len(&task.status.message))
                    .saturating_add(json_len(&m)),
            };
            next.check(items(task))?;
            *kept = next;
            task.status.message = Some(m);
        }
    }
    Ok(())
}

/// Did the SDK fail before any byte left (request serialization, no
/// transport)? Only then is a local error a definite refusal; every other
/// local `INTERNAL_ERROR` (HTTP, body bound, malformed or missing result)
/// arrives after the request may have executed.
fn refused_before_send(e: &A2AError) -> bool {
    e.code == error_code::INTERNAL_ERROR && e.message.starts_with("failed to serialize request")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 183 review 2: a held task keeps its follower past FOLLOW_MAX, sliding with the wait,
    /// but never past start + HOLD_TTL + FOLLOW_MAX; a task not waiting keeps its deadline.
    #[test]
    fn the_follow_deadline_slides_while_held_up_to_the_ceiling() {
        use agent::communication::network::HOLD_TTL;
        let t0 = tokio::time::Instant::now();
        let first = t0 + FOLLOW_MAX;
        let hour = Duration::from_secs(3600);
        assert_eq!(follow_deadline(t0, first, t0 + hour, false), first);
        assert_eq!(
            follow_deadline(t0, first, t0 + hour, true),
            t0 + hour + FOLLOW_MAX,
            "slides while waiting"
        );
        let ceiling = t0 + HOLD_TTL + FOLLOW_MAX;
        assert_eq!(
            follow_deadline(t0, first, t0 + HOLD_TTL + hour, true),
            ceiling,
            "never past the ceiling"
        );
        assert_eq!(
            follow_deadline(t0, ceiling, t0 + 2 * HOLD_TTL, true),
            ceiling
        );
        // Released: the deadline reached so far stands.
        let slid = follow_deadline(t0, first, t0 + hour, true);
        assert_eq!(follow_deadline(t0, slid, t0 + 2 * hour, false), slid);
    }

    fn card(ifaces: Vec<AgentInterface>) -> AgentCard {
        AgentCard {
            name: "remote".into(),
            description: String::new(),
            version: "1".into(),
            supported_interfaces: ifaces,
            capabilities: AgentCapabilities {
                streaming: None,
                push_notifications: None,
                extensions: None,
                extended_agent_card: None,
            },
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            provider: None,
            documentation_url: None,
            icon_url: None,
            security_schemes: None,
            security_requirements: None,
            signatures: None,
        }
    }

    #[test]
    fn interface_must_be_jsonrpc_1_0_on_the_configured_origin() {
        let base = Url::parse("https://lab.example/agents/r").unwrap();
        let ok = AgentInterface::new(
            "https://lab.example/agents/r/rpc",
            TRANSPORT_PROTOCOL_JSONRPC,
        );
        let rest =
            AgentInterface::new("https://lab.example/agents/r", TRANSPORT_PROTOCOL_HTTP_JSON);
        let foreign = AgentInterface::new(
            "https://evil.example/agents/r/rpc",
            TRANSPORT_PROTOCOL_JSONRPC,
        );
        let downgrade = AgentInterface::new(
            "http://lab.example/agents/r/rpc",
            TRANSPORT_PROTOCOL_JSONRPC,
        );
        let creds = AgentInterface::new(
            "https://u:p@lab.example/agents/r/rpc",
            TRANSPORT_PROTOCOL_JSONRPC,
        );
        let mut newer = ok.clone();
        newer.protocol_version = "1.1".into();
        let mut legacy = ok.clone();
        legacy.protocol_version = "0.3".into();

        assert_eq!(
            compatible_interface(&base, &card(vec![rest.clone(), ok.clone()]))
                .unwrap()
                .url,
            ok.url
        );
        for bad in [
            vec![rest],
            vec![foreign],
            vec![downgrade],
            vec![creds.clone()],
            vec![newer],
            vec![legacy],
        ] {
            assert!(matches!(
                compatible_interface(&base, &card(bad)),
                Err(ConnectError::Incompatible(_))
            ));
        }
        // A rejected URL's userinfo (the bearer, reflected) never reaches the error.
        let Err(ConnectError::Incompatible(msg)) = compatible_interface(&base, &card(vec![creds]))
        else {
            panic!("credentials accepted");
        };
        assert!(!msg.contains("u:p"), "{msg}");
        // Default port equivalence.
        let explicit = AgentInterface::new(
            "https://lab.example:443/agents/r/rpc",
            TRANSPORT_PROTOCOL_JSONRPC,
        );
        assert!(compatible_interface(&base, &card(vec![explicit])).is_ok());
    }

    fn task(state: TaskState) -> Task {
        Task {
            id: "t".into(),
            context_id: "c".into(),
            status: TaskStatus {
                state,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        }
    }

    fn artifact(id: &str, text: &str) -> TaskArtifactUpdateEvent {
        TaskArtifactUpdateEvent {
            task_id: "t".into(),
            context_id: "c".into(),
            artifact: Artifact {
                artifact_id: id.into(),
                name: None,
                description: None,
                parts: vec![Part::text(text)],
                metadata: None,
                extensions: None,
            },
            append: None,
            last_chunk: None,
            metadata: None,
        }
    }

    #[test]
    fn apply_enforces_artifact_bound_and_replacement() {
        let mut t = task(TaskState::Working);
        let mut kept = Retained::of(&t).unwrap();
        let big = "x".repeat(ARTIFACT_BYTES_MAX - 10);
        apply(
            &mut t,
            StreamResponse::ArtifactUpdate(artifact("a", &big)),
            &mut kept,
        )
        .unwrap();
        assert_eq!(kept.text, ARTIFACT_BYTES_MAX - 10);
        // Replacing the same artifact with same-size content stays within bound.
        apply(
            &mut t,
            StreamResponse::ArtifactUpdate(artifact("a", &big)),
            &mut kept,
        )
        .unwrap();
        assert_eq!(t.artifacts.as_ref().unwrap().len(), 1);
        assert_eq!(kept.text, ARTIFACT_BYTES_MAX - 10);
        // Appending 11 more bytes crosses it.
        let mut more = artifact("a", "01234567890");
        more.append = Some(true);
        assert!(apply(&mut t, StreamResponse::ArtifactUpdate(more), &mut kept).is_err());
        // A second small artifact is fine below the bound.
        let mut t2 = task(TaskState::Working);
        let mut k2 = Retained::of(&t2).unwrap();
        apply(
            &mut t2,
            StreamResponse::ArtifactUpdate(artifact("a", "one")),
            &mut k2,
        )
        .unwrap();
        apply(
            &mut t2,
            StreamResponse::ArtifactUpdate(artifact("b", "two")),
            &mut k2,
        )
        .unwrap();
        assert_eq!(k2.text, 6);
        assert_eq!(t2.artifacts.as_ref().unwrap().len(), 2);
    }

    /// Empty text costs nothing under the text bound; what it drags along
    /// (metadata, parts, artifacts) is bounded all the same.
    #[test]
    fn apply_bounds_empty_text_growth() {
        // Appends of empty parts with fat metadata: serialized bytes.
        let mut t = task(TaskState::Working);
        let mut kept = Retained::of(&t).unwrap();
        let mut fat = artifact("a", "");
        fat.artifact.parts[0].metadata = Some(
            [(
                "k".to_owned(),
                serde_json::Value::String("m".repeat(60 * 1024)),
            )]
            .into(),
        );
        fat.append = Some(true);
        let mut n = 0;
        while apply(
            &mut t,
            StreamResponse::ArtifactUpdate(fat.clone()),
            &mut kept,
        )
        .is_ok()
        {
            n += 1;
            assert!(n < 100, "empty-text appends never bounded");
        }
        assert_eq!(kept.text, 0);
        assert!(kept.bytes <= TASK_BYTES_MAX);
        assert!(n >= 30, "legal metadata refused early: {n}");
        // Snapshot unchanged by the refused event.
        assert_eq!(t.artifacts.as_ref().unwrap()[0].parts.len(), n);

        // Distinct empty artifacts: item count.
        let mut t = task(TaskState::Working);
        let mut kept = Retained::of(&t).unwrap();
        let mut n = 0;
        while apply(
            &mut t,
            StreamResponse::ArtifactUpdate(artifact(&n.to_string(), "")),
            &mut kept,
        )
        .is_ok()
        {
            n += 1;
            assert!(n <= TASK_ITEMS_MAX, "empty artifacts never bounded");
        }
        assert_eq!(t.artifacts.as_ref().unwrap().len(), n);
        assert_eq!(items(&t), 2 * n);

        // Same for a whole snapshot: history that the text bound cannot see.
        let mut t = task(TaskState::Working);
        t.history = Some(vec![Message::new(
            Role::Agent,
            vec![Part::text("h".repeat(TASK_BYTES_MAX))],
        )]);
        assert!(Retained::of(&t).is_err());
        t.history = Some(vec![Message::new(Role::Agent, vec![]); TASK_ITEMS_MAX + 1]);
        assert!(Retained::of(&t).is_err());
        // A legal large text artifact still fits under both bounds.
        t.history = None;
        t.artifacts = Some(vec![
            artifact("a", &"x".repeat(ARTIFACT_BYTES_MAX)).artifact,
        ]);
        assert_eq!(Retained::of(&t).unwrap().text, ARTIFACT_BYTES_MAX);
    }

    #[test]
    fn apply_refuses_binary_and_oversized_status() {
        let mut t = task(TaskState::Working);
        let mut bytes = Retained::default();
        let mut bin = artifact("a", "");
        bin.artifact.parts = vec![Part {
            content: PartContent::Raw(vec![0, 1, 2]),
            filename: None,
            media_type: Some("application/octet-stream".into()),
            metadata: None,
        }];
        assert!(apply(&mut t, StreamResponse::ArtifactUpdate(bin), &mut bytes).is_err());
        let status = TaskStatusUpdateEvent {
            task_id: "t".into(),
            context_id: "c".into(),
            status: TaskStatus {
                state: TaskState::Working,
                message: Some(Message::new(
                    Role::Agent,
                    vec![Part::text("y".repeat(STATUS_TEXT_MAX + 1))],
                )),
                timestamp: None,
            },
            metadata: None,
        };
        assert!(apply(&mut t, StreamResponse::StatusUpdate(status), &mut bytes).is_err());
        assert_eq!(t.status.state, TaskState::Working);
    }

    #[test]
    fn fingerprint_changes_only_with_visible_state() {
        let mut t = task(TaskState::Working);
        let a = fingerprint(&t);
        assert_eq!(a, fingerprint(&t.clone()));
        t.status.state = TaskState::AuthRequired;
        let b = fingerprint(&t);
        assert_ne!(a, b);
        // Out-of-band resume after AUTH_REQUIRED is a new notice.
        t.status.state = TaskState::Working;
        assert_eq!(fingerprint(&t), a);
        let mut kept = Retained::default();
        apply(
            &mut t,
            StreamResponse::ArtifactUpdate(artifact("a", "done")),
            &mut kept,
        )
        .unwrap();
        assert_ne!(fingerprint(&t), a);
    }

    /// Only a failure proven to precede the request is a refusal; every other
    /// local error may follow an executed request.
    #[test]
    fn only_pre_send_failures_are_refusals() {
        assert!(refused_before_send(&A2AError::internal(
            "failed to serialize request as ProtoJSON: x"
        )));
        for after in [
            "HTTP request failed: connection reset",
            "failed to parse JSON-RPC response: eof",
            "response body exceeds 2097152 bytes",
            "JSON-RPC response missing result",
            "failed to deserialize result: missing field",
        ] {
            assert!(!refused_before_send(&A2AError::internal(after)), "{after}");
        }
        assert!(!refused_before_send(&A2AError::task_not_found("t")));
    }
}
