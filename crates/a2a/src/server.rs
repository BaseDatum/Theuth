//! Server side (design doc 50 §4, §6): the SDK's `agent_card_router` and
//! `jsonrpc_router` per export, behind wth's HTTP layer (bearer auth →
//! trusted principal header, no CORS, private cards, 64 KiB body bound,
//! per-peer rate/concurrency caps), with a [`RequestHandler`] that only
//! translates typed requests into agent commands. No task store lives here.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a2a_server::agent_card::{StaticAgentCard, agent_card_router};
use a2a_server::jsonrpc::jsonrpc_router;
use a2a_server::{RequestHandler, ServiceParams};
use a2a_types::*;
use agent::Agents;
use agent::communication::network::{
    A2aCancel, A2aGet, A2aList, A2aPrincipal, A2aSend, A2aStream, A2aSubscribe, TaskStream,
};
use async_trait::async_trait;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use futures::stream::{self, BoxStream, StreamExt};
use kameo::actor::ActorRef;
use kameo::message::Message as ActorMessage;
use kameo::reply::Reply;
use parking_lot::{Mutex, RwLock};
use runtime::SessionId;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tower_http::limit::RequestBodyLimitLayer;
use url::Url;
use uuid::Uuid;

use agent::communication::SharedPolicy;

use crate::config::{Staged, Token};

/// Trusted seam between the HTTP layer and the handler (§6.1): the verified
/// binding UUID. The released `jsonrpc_router` hands the handler only the
/// lowercased header map, so the layer strips any inbound copy first.
pub const PRINCIPAL_HEADER: &str = "x-wth-a2a-principal";
/// `a2a_types::SVC_PARAM_VERSION` as axum lowercases it.
const VERSION_PARAM: &str = "a2a-version";
/// Outer JSON body bound (§8); the SDK's 10 MiB inner limit never applies.
pub const BODY_LIMIT: usize = 64 * 1024;
/// SSE subscribers across all tasks (§8).
pub const SUBSCRIBERS_MAX: usize = 128;
/// A protected stream that stays silent this long is closed (§8).
pub const STREAM_IDLE: Duration = Duration::from_secs(120);
/// Admitted requests per second per authenticated peer (§8).
const RATE_PER_SEC: f64 = 10.0;
/// Concurrent HTTP handlers per peer (§8).
const CONCURRENT_PER_PEER: usize = 8;

/// One authenticated remote identity as the HTTP layer knows it.
pub struct Principal {
    /// Store binding UUID; the only identity the handler sees.
    pub binding: Uuid,
    /// Owner alias, for logs and status.
    pub peer: String,
    /// Grant epoch (§6.1): cancelled by the plugin once this peer's
    /// credentials, exports or roots changed. Every admission and every
    /// emission under this principal is bound to it; the binding id alone
    /// authorizes nothing after that.
    pub epoch: CancellationToken,
    exports: HashSet<String>,
    concurrent: Arc<Semaphore>,
    bucket: Mutex<(f64, Instant)>,
}

impl Principal {
    fn new(
        binding: Uuid,
        peer: String,
        exports: HashSet<String>,
        epoch: CancellationToken,
    ) -> Self {
        Self {
            binding,
            peer,
            epoch,
            exports,
            concurrent: Arc::new(Semaphore::new(CONCURRENT_PER_PEER)),
            bucket: Mutex::new((RATE_PER_SEC, Instant::now())),
        }
    }

    /// Token bucket: `RATE_PER_SEC` sustained, same burst.
    fn admit_rate(&self) -> bool {
        let mut b = self.bucket.lock();
        let now = Instant::now();
        b.0 = (b.0 + now.duration_since(b.1).as_secs_f64() * RATE_PER_SEC).min(RATE_PER_SEC);
        b.1 = now;
        if b.0 >= 1.0 {
            b.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The installed configuration, replaced whole on reload. Requests read the
/// current one; a reload never edits a live instance.
pub struct Live {
    /// Card + RPC routes for every export, nested under `/agents/<name>`.
    router: Router,
    /// Inbound bearer → principal. Linear, constant-time per entry: a few peers.
    principals: Vec<(Token, Arc<Principal>)>,
    /// Export name → published session (for the 404-not-401 decision and
    /// the live enablement check).
    exports: HashMap<String, SessionId>,
    /// The agent's live availability predicate: a disabled session's export
    /// answers nothing, whatever the cached routes say (§2.2, §8).
    policy: Option<SharedPolicy>,
    /// What was validated, for status.
    pub staged: Staged,
    /// Alias → binding UUID as committed through the agent.
    pub bindings: BTreeMap<String, Uuid>,
}

/// Swappable pointer the HTTP layer dereferences per request.
pub type Shared = Arc<RwLock<Arc<Live>>>;

impl Live {
    /// Nothing installed: every request is 404/401.
    pub fn empty() -> Self {
        Self {
            router: Router::new(),
            principals: Vec::new(),
            exports: HashMap::new(),
            policy: None,
            staged: Staged::default(),
            bindings: BTreeMap::new(),
        }
    }

    /// Build routes and the auth map from a validated set, its bindings and
    /// grant epochs. `base` is the advertised URL (§6.1); `subscribers` the
    /// global SSE cap; `policy` the agent's live switch consulted on every
    /// request.
    pub fn build(
        staged: Staged,
        bindings: BTreeMap<String, Uuid>,
        epochs: &BTreeMap<String, CancellationToken>,
        base: &Url,
        agents: ActorRef<Agents>,
        subscribers: Arc<Semaphore>,
        policy: SharedPolicy,
    ) -> Self {
        let principals: Vec<(Token, Arc<Principal>)> = staged
            .peers
            .values()
            .filter_map(|p| {
                let token = p.inbound.clone()?;
                let binding = *bindings.get(&p.name)?;
                Some((
                    token,
                    Arc::new(Principal::new(
                        binding,
                        p.name.clone(),
                        p.exports.iter().cloned().collect(),
                        epochs.get(&p.name)?.clone(),
                    )),
                ))
            })
            .collect();
        let epochs: Arc<HashMap<Uuid, CancellationToken>> = Arc::new(
            principals
                .iter()
                .map(|(_, p)| (p.binding, p.epoch.clone()))
                .collect(),
        );
        let mut router = Router::new();
        for export in staged.exports.values() {
            let prefix = format!("/agents/{}", export.name);
            let handler = Arc::new(Handler {
                export: export.name.clone(),
                session: export.session,
                agents: agents.clone(),
                subscribers: subscribers.clone(),
                epochs: epochs.clone(),
            });
            let card = Arc::new(StaticAgentCard::new(card(
                &format!("{}{prefix}/rpc", base.as_str().trim_end_matches('/')),
                &export.title,
                &export.description,
            )));
            let export_router = Router::new()
                .merge(agent_card_router(card).layer(middleware::map_response(private_card)))
                .nest_service("/rpc", jsonrpc_router(handler))
                .layer(RequestBodyLimitLayer::new(BODY_LIMIT));
            router = router.nest(&prefix, export_router);
        }
        Self {
            router,
            principals,
            exports: staged
                .exports
                .values()
                .map(|e| (e.name.clone(), e.session))
                .collect(),
            policy: Some(policy),
            staged,
            bindings,
        }
    }

    fn authenticate(&self, bearer: &str) -> Option<&Arc<Principal>> {
        self.principals
            .iter()
            .find(|(t, _)| ct_eq(t.as_str().as_bytes(), bearer.as_bytes()))
            .map(|(_, p)| p)
    }

    /// The export exists and its session is effectively enabled right now.
    fn open(&self, export: &str) -> bool {
        let Some(session) = self.exports.get(export) else {
            return false;
        };
        self.policy
            .as_ref()
            .is_some_and(|p| p.0.read().enabled(*session).is_ok())
    }
}

/// Constant-time byte equality; the token map is network-facing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The one HTTP service: authenticate, gate, then dispatch into the current
/// [`Live`] router. A reload swaps the pointer; in-flight requests finish on
/// the routes they started with unless the reload retired their grant, in
/// which case they end unauthorized instead of emitting anything.
pub fn app(shared: Shared) -> Router {
    Router::new().fallback(dispatch).with_state(shared)
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
    )
        .into_response()
}

async fn dispatch(State(shared): State<Shared>, mut req: Request) -> Response {
    let live = shared.read().clone();
    // Any inbound copy of the trusted seam is an attack or a bug; never forward it.
    req.headers_mut().remove(PRINCIPAL_HEADER);
    let export = req
        .uri()
        .path()
        .strip_prefix("/agents/")
        .and_then(|rest| rest.split('/').next())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty());
    // Credentials are checked before existence: an unauthenticated caller
    // learns nothing about which exports exist (§6.1).
    let Some(principal) = bearer.and_then(|b| live.authenticate(b)).cloned() else {
        return unauthorized();
    };
    // Granted, existing and effectively enabled right now, or not found: a
    // disabled export is not disclosed and its cached routes never answer.
    let Some(export) = export.filter(|e| principal.exports.contains(e) && live.open(e)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !principal.admit_rate() {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(_permit) = principal.concurrent.clone().try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, HeaderValue::from_static("1"))],
        )
            .into_response();
    };
    let Ok(v) = HeaderValue::from_str(&principal.binding.to_string()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    req.headers_mut().insert(PRINCIPAL_HEADER, v);
    tracing::debug!(peer = %principal.peer, %export, method = %req.method(), "a2a request");
    tokio::select! {
        biased;
        () = principal.epoch.cancelled() => unauthorized(),
        res = live.router.clone().oneshot(req) => match res {
            Ok(res) => res,
            Err(never) => match never {},
        },
    }
}

/// The SDK card route unconditionally adds `Access-Control-Allow-Origin`
/// (`*`, or the request origin plus `allow-credentials`). This profile
/// rejects browser origins and marks protected cards private (§6.1).
async fn private_card(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    h.remove(header::ACCESS_CONTROL_ALLOW_CREDENTIALS);
    h.remove(header::VARY);
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    res
}

/// Export card (§6.2): one JSONRPC/1.0 interface, text in/out, explicit
/// capability booleans, bearer required. No tool inventory, title or history.
fn card(rpc_url: &str, title: &str, description: &str) -> AgentCard {
    AgentCard {
        name: title.to_owned(),
        description: description.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        supported_interfaces: vec![AgentInterface::new(rpc_url, TRANSPORT_PROTOCOL_JSONRPC)],
        capabilities: AgentCapabilities {
            streaming: Some(true),
            push_notifications: Some(false),
            extensions: None,
            extended_agent_card: Some(false),
        },
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into()],
        skills: vec![AgentSkill {
            id: "chat".into(),
            name: title.to_owned(),
            description: description.to_owned(),
            tags: vec!["text".into()],
            examples: None,
            input_modes: None,
            output_modes: None,
            security_requirements: None,
        }],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: Some(HashMap::from([(
            "bearer".to_owned(),
            SecurityScheme::HttpAuth(HttpAuthSecurityScheme {
                scheme: "bearer".into(),
                description: Some("Configured peer credential".into()),
                bearer_format: None,
            }),
        )])),
        security_requirements: Some(vec![HashMap::from([("bearer".to_owned(), Vec::new())])]),
        signatures: None,
    }
}

/// Version negotiation (§3.3), the handler's duty on every method: the
/// released router never reads `A2A-Version`. Absent/empty means 0.3 per
/// spec 3.6.2, which this profile does not serve. Patch versions tolerated.
pub fn check_version(params: &ServiceParams) -> Result<(), A2AError> {
    let requested = params
        .get(VERSION_PARAM)
        .and_then(|v| v.first())
        .map_or("", String::as_str);
    let requested = if requested.is_empty() {
        "0.3"
    } else {
        requested
    };
    let mut parts = requested.splitn(3, '.');
    let (major, minor) = (parts.next().unwrap_or(""), parts.next().unwrap_or("0"));
    if major == "1" && minor == "0" {
        Ok(())
    } else {
        Err(A2AError::version_not_supported(requested))
    }
}

/// `a2a_server::task_store::apply_history_length` is `pub(crate)`; same rule.
pub fn trim_history(task: &mut Task, history_length: Option<i32>) {
    let Some(n) = history_length else { return };
    if n <= 0 {
        task.history = None;
        return;
    }
    if let Some(h) = task.history.as_mut() {
        let keep = n as usize;
        if h.len() > keep {
            h.drain(..h.len() - keep);
        }
    }
}

/// Protocol-level request validation (§6.2): ProtoJSON defaults a missing
/// `message` to an empty one, so emptiness is checked here; only text parts;
/// no push config, no required extensions.
pub fn validate_send(req: &SendMessageRequest) -> Result<(), A2AError> {
    let m = &req.message;
    if m.message_id.trim().is_empty() {
        return Err(A2AError::invalid_params("message.messageId is required"));
    }
    if m.parts.is_empty() {
        return Err(A2AError::invalid_params("message.parts must not be empty"));
    }
    if m.extensions.as_ref().is_some_and(|e| !e.is_empty()) {
        return Err(A2AError::new(
            error_code::EXTENSION_SUPPORT_REQUIRED,
            "extensions are not supported",
        ));
    }
    if m.parts.iter().any(|p| p.as_text().is_none()) {
        return Err(A2AError::content_type_not_supported());
    }
    if let Some(cfg) = &req.configuration {
        if cfg.task_push_notification_config.is_some() {
            return Err(A2AError::push_notification_not_supported());
        }
        if cfg
            .accepted_output_modes
            .as_ref()
            .is_some_and(|modes| !modes.is_empty() && !modes.iter().any(|m| accepts_text(m)))
        {
            return Err(A2AError::content_type_not_supported());
        }
    }
    Ok(())
}

fn accepts_text(mode: &str) -> bool {
    let m = mode.trim();
    m == "*/*" || m == "text/*" || m.starts_with("text/plain") || m == "text"
}

/// Application seam per export: typed requests → agent commands (§4).
struct Handler {
    export: String,
    session: SessionId,
    agents: ActorRef<Agents>,
    subscribers: Arc<Semaphore>,
    /// Binding → grant epoch of the configuration these routes belong to.
    epochs: Arc<HashMap<Uuid, CancellationToken>>,
}

fn revoked() -> A2AError {
    A2AError::unsupported_operation("export unavailable: peer grant revoked")
}

impl Handler {
    fn admit(&self, params: &ServiceParams) -> Result<A2aPrincipal, A2AError> {
        check_version(params)?;
        let binding = params
            .get(PRINCIPAL_HEADER)
            .and_then(|v| v.first())
            .and_then(|v| v.parse::<Uuid>().ok())
            .ok_or_else(|| A2AError::internal("authentication layer did not run"))?;
        let route = self
            .epochs
            .get(&binding)
            .ok_or_else(|| A2AError::internal("authentication layer did not run"))?;
        if route.is_cancelled() {
            return Err(revoked());
        }
        Ok(A2aPrincipal {
            binding,
            export: self.export.clone(),
            session: self.session,
            route: route.clone(),
        })
    }

    /// Ask the agent; the supervisor answers some commands through a
    /// `DelegatedReply`, so the bound is on the reply's `Ok`/`Error`, not its shape.
    async fn ask<M>(
        &self,
        msg: M,
    ) -> Result<<<Agents as ActorMessage<M>>::Reply as Reply>::Ok, A2AError>
    where
        Agents: ActorMessage<M>,
        <Agents as ActorMessage<M>>::Reply: Reply<Error = A2AError>,
        M: Send + 'static,
    {
        match self.agents.ask(msg).await {
            Ok(r) => Ok(r),
            Err(kameo::error::SendError::HandlerError(e)) => Err(e),
            Err(e) => {
                tracing::warn!(export = %self.export, "agent unavailable: {e}");
                Err(A2AError::internal("agent unavailable"))
            }
        }
    }

    fn bounded(&self, principal: &A2aPrincipal, inner: TaskStream) -> Result<TaskStream, A2AError> {
        let permit = self
            .subscribers
            .clone()
            .try_acquire_owned()
            .map_err(|_| A2AError::internal("too many open streams; retry later"))?;
        Ok(bounded(inner, permit, principal.route.clone()))
    }
}

/// Idle deadline, grant epoch and global subscriber slot for one protected
/// stream (§8). The agent ends the stream on revocation too; this bounds the
/// socket so no event of a retired grant is written after the swap.
fn bounded(
    inner: TaskStream,
    permit: OwnedSemaphorePermit,
    epoch: CancellationToken,
) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
    Box::pin(stream::unfold(
        (inner, permit, epoch, false),
        |(mut inner, permit, epoch, done)| async move {
            if done {
                return None;
            }
            let next = tokio::select! {
                biased;
                () = epoch.cancelled() => return Some((Err(revoked()), (inner, permit, epoch, true))),
                r = tokio::time::timeout(STREAM_IDLE, inner.next()) => r,
            };
            match next {
                Ok(Some(item)) => Some((item, (inner, permit, epoch, false))),
                Ok(None) => None,
                Err(_) => Some((
                    Err(A2AError::internal("stream idle deadline reached")),
                    (inner, permit, epoch, true),
                )),
            }
        },
    ))
}

#[async_trait]
impl RequestHandler for Handler {
    async fn send_message(
        &self,
        params: &ServiceParams,
        req: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2AError> {
        let principal = self.admit(params)?;
        validate_send(&req)?;
        let history_length = req.configuration.as_ref().and_then(|c| c.history_length);
        let mut response = self
            .ask(A2aSend {
                principal,
                request: req,
            })
            .await?;
        if let SendMessageResponse::Task(task) = &mut response {
            trim_history(task, history_length);
        }
        Ok(response)
    }

    async fn send_streaming_message(
        &self,
        params: &ServiceParams,
        req: SendMessageRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        let principal = self.admit(params)?;
        validate_send(&req)?;
        let stream = self
            .ask(A2aStream {
                principal: principal.clone(),
                request: req,
            })
            .await?;
        self.bounded(&principal, stream)
    }

    async fn get_task(
        &self,
        params: &ServiceParams,
        req: GetTaskRequest,
    ) -> Result<Task, A2AError> {
        let principal = self.admit(params)?;
        let history_length = req.history_length;
        let mut task = self
            .ask(A2aGet {
                principal,
                request: req,
            })
            .await?;
        trim_history(&mut task, history_length);
        Ok(task)
    }

    async fn list_tasks(
        &self,
        params: &ServiceParams,
        req: ListTasksRequest,
    ) -> Result<ListTasksResponse, A2AError> {
        let principal = self.admit(params)?;
        let history_length = req.history_length;
        let include_artifacts = req.include_artifacts == Some(true);
        let mut page = self
            .ask(A2aList {
                principal,
                request: req,
            })
            .await?;
        for t in &mut page.tasks {
            trim_history(t, history_length);
            if !include_artifacts {
                t.artifacts = None;
            }
        }
        Ok(page)
    }

    async fn cancel_task(
        &self,
        params: &ServiceParams,
        req: CancelTaskRequest,
    ) -> Result<Task, A2AError> {
        let principal = self.admit(params)?;
        self.ask(A2aCancel {
            principal,
            request: req,
        })
        .await
    }

    async fn subscribe_to_task(
        &self,
        params: &ServiceParams,
        req: SubscribeToTaskRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        let principal = self.admit(params)?;
        let stream = self
            .ask(A2aSubscribe {
                principal: principal.clone(),
                request: req,
            })
            .await?;
        self.bounded(&principal, stream)
    }

    async fn create_push_config(
        &self,
        params: &ServiceParams,
        _req: TaskPushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        self.admit(params)?;
        Err(A2AError::push_notification_not_supported())
    }

    async fn get_push_config(
        &self,
        params: &ServiceParams,
        _req: GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        self.admit(params)?;
        Err(A2AError::push_notification_not_supported())
    }

    async fn list_push_configs(
        &self,
        params: &ServiceParams,
        _req: ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, A2AError> {
        self.admit(params)?;
        Err(A2AError::push_notification_not_supported())
    }

    async fn delete_push_config(
        &self,
        params: &ServiceParams,
        _req: DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2AError> {
        self.admit(params)?;
        Err(A2AError::push_notification_not_supported())
    }

    async fn get_extended_agent_card(
        &self,
        params: &ServiceParams,
        _req: GetExtendedAgentCardRequest,
    ) -> Result<AgentCard, A2AError> {
        self.admit(params)?;
        Err(A2AError::unsupported_operation(
            "extended agent card not supported",
        ))
    }
}

/// Serve `app` on an already bound listener until `shutdown`.
pub async fn serve(
    listener: tokio::net::TcpListener,
    shared: Shared,
    shutdown: tokio_util::sync::CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, app(shared))
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(version: Option<&str>) -> ServiceParams {
        let mut p = ServiceParams::new();
        if let Some(v) = version {
            p.insert(VERSION_PARAM.into(), vec![v.into()]);
        }
        p
    }

    /// A `Live` whose export router echoes the trusted principal header back
    /// as `x-seen-principal`, so the seam can be observed end to end.
    fn live_for_test(
        tokens: &[(&str, &str, &[&str])],
        exports: &[(&str, SessionId)],
        policy: SharedPolicy,
    ) -> Shared {
        use axum::http::HeaderMap;
        use axum::routing::post;
        async fn echo(headers: HeaderMap) -> Response {
            let seen = headers
                .get(PRINCIPAL_HEADER)
                .cloned()
                .unwrap_or_else(|| HeaderValue::from_static("none"));
            (StatusCode::OK, [("x-seen-principal", seen)]).into_response()
        }
        let mut router = Router::new();
        for (e, _) in exports {
            router = router.nest(
                &format!("/agents/{e}"),
                Router::new().nest_service("/rpc", post(echo)),
            );
        }
        let principals = tokens
            .iter()
            .enumerate()
            .map(|(index, (tok, peer, grants))| {
                (
                    Token::for_test(tok),
                    Arc::new(Principal::new(
                        Uuid::from_u128(index as u128 + 1),
                        (*peer).to_owned(),
                        grants.iter().map(|g| (*g).to_owned()).collect(),
                        CancellationToken::new(),
                    )),
                )
            })
            .collect();
        Arc::new(RwLock::new(Arc::new(Live {
            router,
            principals,
            exports: exports.iter().map(|(e, s)| ((*e).to_owned(), *s)).collect(),
            policy: Some(policy),
            staged: Staged::default(),
            bindings: BTreeMap::new(),
        })))
    }

    async fn call(
        shared: &Shared,
        path: &str,
        bearer: Option<&str>,
        forged: Option<&str>,
    ) -> Response {
        let mut req = Request::builder().method("POST").uri(path);
        if let Some(b) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {b}"));
        }
        if let Some(f) = forged {
            req = req.header(PRINCIPAL_HEADER, f);
        }
        app(shared.clone())
            .oneshot(req.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn auth_layer_gates_and_seams() {
        use agent::communication::{PeerScope, Policy};
        let (research, other) = (SessionId::new(), SessionId::new());
        let mut policy = Policy::new(true, "test");
        policy.record_root(research, PeerScope::Owner).unwrap();
        policy.record_root(other, PeerScope::Owner).unwrap();
        let policy = SharedPolicy::new(policy);
        let shared = live_for_test(
            &[("tok-a", "a", &["research"]), ("tok-b", "b", &["other"])],
            &[("research", research), ("other", other)],
            policy.clone(),
        );
        let a_binding = Uuid::from_u128(1).to_string();

        // No credential: 401 with a challenge, whatever the path.
        let r = call(&shared, "/agents/research/rpc", None, None).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(r.headers()[header::WWW_AUTHENTICATE], "Bearer");
        let r = call(&shared, "/agents/nope/rpc", None, None).await;
        assert_eq!(
            r.status(),
            StatusCode::UNAUTHORIZED,
            "unknown export is not disclosed before auth"
        );

        // Wrong credential: 401.
        assert_eq!(
            call(&shared, "/agents/research/rpc", Some("tok-x"), None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );

        // Right credential, ungranted or unknown export: same 404.
        assert_eq!(
            call(&shared, "/agents/other/rpc", Some("tok-a"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&shared, "/agents/nope/rpc", Some("tok-a"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&shared, "/", Some("tok-a"), None).await.status(),
            StatusCode::NOT_FOUND
        );

        // Granted: reaches the export router with the verified binding; a
        // forged inbound seam header is replaced, never forwarded.
        let r = call(
            &shared,
            "/agents/research/rpc",
            Some("tok-a"),
            Some("00000000-0000-0000-0000-00000000dead"),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["x-seen-principal"], a_binding.as_str());
        // Trailing slash (the kit's httpx posts it) is served too.
        assert_eq!(
            call(&shared, "/agents/research/rpc/", Some("tok-a"), None)
                .await
                .status(),
            StatusCode::OK
        );

        // Live policy: the export's session opting out, or the global switch
        // going off, closes the cached routes at once (no reload needed).
        policy.0.write().set_session(research, false).unwrap();
        assert_eq!(
            call(&shared, "/agents/research/rpc", Some("tok-a"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&shared, "/agents/other/rpc", Some("tok-b"), None)
                .await
                .status(),
            StatusCode::OK
        );
        policy.0.write().set_session(research, true).unwrap();
        assert_eq!(
            call(&shared, "/agents/research/rpc", Some("tok-a"), None)
                .await
                .status(),
            StatusCode::OK
        );
        policy.0.write().set_global(false, "test");
        assert_eq!(
            call(&shared, "/agents/other/rpc", Some("tok-b"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        policy.0.write().set_global(true, "test");

        // A retired grant: the credential still matches the cached map the
        // in-flight request holds, but nothing under it is dispatched.
        shared.read().principals[1].1.epoch.cancel();
        assert_eq!(
            call(&shared, "/agents/other/rpc", Some("tok-b"), None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&shared, "/agents/research/rpc", Some("tok-a"), None)
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn bounded_stream_ends_with_revocation_when_grant_retired() {
        let epoch = CancellationToken::new();
        let permit = Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap();
        let mut s = bounded(Box::pin(stream::pending()), permit, epoch.clone());
        epoch.cancel();
        let first = s.next().await.expect("one terminal error").unwrap_err();
        assert_eq!(first.code, error_code::UNSUPPORTED_OPERATION);
        assert!(first.message.contains("revoked"));
        assert!(
            s.next().await.is_none(),
            "no emission after the revocation error"
        );
    }

    #[test]
    fn version_negotiation() {
        assert!(check_version(&params(Some("1.0"))).is_ok());
        assert!(check_version(&params(Some("1.0.7"))).is_ok());
        let absent = check_version(&params(None)).unwrap_err();
        assert_eq!(absent.code, error_code::VERSION_NOT_SUPPORTED);
        assert!(absent.message.contains("0.3"));
        assert_eq!(
            check_version(&params(Some("0.3"))).unwrap_err().code,
            error_code::VERSION_NOT_SUPPORTED
        );
        assert_eq!(
            check_version(&params(Some("2.0"))).unwrap_err().code,
            error_code::VERSION_NOT_SUPPORTED
        );
    }

    fn send(message: Message) -> SendMessageRequest {
        SendMessageRequest {
            message,
            configuration: None,
            metadata: None,
            tenant: None,
        }
    }

    #[test]
    fn send_validation() {
        let ok = Message::new(Role::User, vec![Part::text("hi")]);
        assert!(validate_send(&send(ok.clone())).is_ok());

        let mut empty_id = ok.clone();
        empty_id.message_id.clear();
        assert_eq!(
            validate_send(&send(empty_id)).unwrap_err().code,
            error_code::INVALID_PARAMS
        );

        let no_parts = Message::new(Role::User, vec![]);
        assert_eq!(
            validate_send(&send(no_parts)).unwrap_err().code,
            error_code::INVALID_PARAMS
        );

        let file = Message::new(
            Role::User,
            vec![Part {
                content: PartContent::Url("file:///etc/passwd".into()),
                filename: None,
                media_type: Some("text/plain".into()),
                metadata: None,
            }],
        );
        assert_eq!(
            validate_send(&send(file)).unwrap_err().code,
            error_code::CONTENT_TYPE_NOT_SUPPORTED
        );

        let mut ext = ok.clone();
        ext.extensions = Some(vec!["https://example/ext".into()]);
        assert_eq!(
            validate_send(&send(ext)).unwrap_err().code,
            error_code::EXTENSION_SUPPORT_REQUIRED
        );

        let mut push = send(ok.clone());
        push.configuration = Some(SendMessageConfiguration {
            accepted_output_modes: None,
            task_push_notification_config: Some(TaskPushNotificationConfig {
                url: "https://evil.example/hook".into(),
                id: None,
                task_id: String::new(),
                token: None,
                authentication: None,
                tenant: None,
            }),
            history_length: None,
            return_immediately: None,
        });
        assert_eq!(
            validate_send(&push).unwrap_err().code,
            error_code::PUSH_NOTIFICATION_NOT_SUPPORTED
        );

        let mut modes = send(ok);
        modes.configuration = Some(SendMessageConfiguration {
            accepted_output_modes: Some(vec!["image/png".into()]),
            task_push_notification_config: None,
            history_length: None,
            return_immediately: None,
        });
        assert_eq!(
            validate_send(&modes).unwrap_err().code,
            error_code::CONTENT_TYPE_NOT_SUPPORTED
        );
    }

    #[test]
    fn history_rule() {
        let mut t = Task {
            id: "t".into(),
            context_id: "c".into(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: Some(vec![
                Message::new(Role::User, vec![Part::text("1")]),
                Message::new(Role::Agent, vec![Part::text("2")]),
                Message::new(Role::User, vec![Part::text("3")]),
            ]),
            metadata: None,
        };
        trim_history(&mut t, None);
        assert_eq!(t.history.as_ref().unwrap().len(), 3);
        trim_history(&mut t, Some(1));
        assert_eq!(t.history.as_ref().unwrap()[0].text(), Some("3"));
        trim_history(&mut t, Some(0));
        assert!(t.history.is_none());
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn rate_bucket_refuses_burst_beyond_ten() {
        let p = Principal::new(
            Uuid::nil(),
            "p".into(),
            HashSet::new(),
            CancellationToken::new(),
        );
        let admitted = (0..20).filter(|_| p.admit_rate()).count();
        assert_eq!(admitted, 10);
    }
}
