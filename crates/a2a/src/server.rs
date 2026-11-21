//! Server side (design doc 50 §4, §6): the SDK's `agent_card_router` and
//! `jsonrpc_router` per export, behind theuth's HTTP layer (bearer auth →
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
use agent::communication::network::access::{SESSION_ROUTE, session_route};
use agent::communication::network::{
    A2aCancel, A2aGet, A2aList, A2aPrincipal, A2aReturn, A2aSend, A2aStream, A2aSubscribe,
    RETURN_ROUTE, TaskStream,
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
pub const PRINCIPAL_HEADER: &str = "x-theuth-a2a-principal";
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
    /// 183 §7.2: contact endpoint id → principal (`theuth/a2a/1`, no bearer).
    contacts: HashMap<String, Arc<Principal>>,
    /// Export name → published session (for the 404-not-401 decision and
    /// the live enablement check).
    exports: HashMap<String, SessionId>,
    /// 183 §7.6: agent export names (each contact's inbox is resolved per request).
    agents: HashSet<String>,
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

/// 183 §7.2: the caller of a `theuth/a2a/1` stream, put on the request by the reach. A
/// request extension, so no HTTP client can set it; it replaces the bearer step.
#[derive(Clone, Debug)]
pub struct ContactCaller(pub String);

/// 191 (D3): `forward::route` charged this contact call already (`Live::admit_contact`: one
/// rate token, and the concurrency permit it holds across its router run and every forward);
/// [`dispatch`] charges it nothing again. A request extension, so no HTTP client can set it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Charged;

/// 183 §7.6: what a contact principal carries into admission.
#[derive(Clone, Debug)]
struct ContactGrant {
    alias: String,
    facts: crate::config::ContactFacts,
    /// 200 (D1): this contact has full access (`Peer.full_access`), D4 (2).
    full_access: bool,
}

impl Live {
    /// 191 (D2): `staged` listed (`contact_list`) but nothing installed: no binding, no
    /// route, no principal; the gate admits no contact.
    pub fn display(staged: Staged) -> Self {
        Self {
            staged,
            ..Self::empty()
        }
    }
}

impl Live {
    /// Nothing installed: every request is 404/401.
    pub fn empty() -> Self {
        Self {
            router: Router::new(),
            principals: Vec::new(),
            contacts: HashMap::new(),
            exports: HashMap::new(),
            agents: HashSet::new(),
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
        let principal = |p: &crate::config::Peer| {
            Some(Arc::new(Principal::new(
                *bindings.get(&p.name)?,
                p.name.clone(),
                p.exports.iter().cloned().collect(),
                epochs.get(&p.name)?.clone(),
            )))
        };
        let principals: Vec<(Token, Arc<Principal>)> = staged
            .peers
            .values()
            .filter_map(|p| Some((p.inbound.clone()?, principal(p)?)))
            .collect();
        // 191 (D2): every anchor of a contact's group is its principal; a denied or removed
        // endpoint never is.
        let contacts: HashMap<String, Arc<Principal>> = staged
            .peers
            .values()
            .filter(|p| p.eid.is_some())
            .filter_map(|p| {
                let principal = principal(p)?;
                let eids = staged
                    .contacts
                    .get(&p.name)
                    .map_or_else(|| p.eid.iter().cloned().collect(), |f| f.eids.clone());
                Some(eids.into_iter().map(move |e| (e, principal.clone())))
            })
            .flatten()
            .filter(|(e, _)| !staged.deny.contains(e) && !staged.removed.contains(e))
            .collect();
        let epochs: Arc<HashMap<Uuid, CancellationToken>> = Arc::new(
            principals
                .iter()
                .map(|(_, p)| p)
                .chain(contacts.values())
                .map(|p| (p.binding, p.epoch.clone()))
                .collect(),
        );
        let grants: Arc<HashMap<Uuid, ContactGrant>> = Arc::new(
            contacts
                .values()
                .filter_map(|p| {
                    let facts = staged.contacts.get(&p.peer)?.clone();
                    Some((
                        p.binding,
                        ContactGrant {
                            alias: p.peer.clone(),
                            facts,
                            full_access: staged.peers.get(&p.peer).is_some_and(|x| x.full_access),
                        },
                    ))
                })
                .collect(),
        );
        let targets = staged
            .exports
            .values()
            .map(|e| (&e.name, Target::Chat(e.session), &e.title, &e.description))
            .chain(staged.agents.values().map(|a| {
                (
                    &a.name,
                    Target::Agent(a.agent.clone(), a.home.clone()),
                    &a.title,
                    &a.description,
                )
            }));
        let mut router = Router::new();
        for (name, target, title, description) in targets {
            let prefix = format!("/agents/{name}");
            let handler = Arc::new(Handler {
                export: name.clone(),
                export_id: staged.export_ids.get(name).cloned(),
                target,
                agents: agents.clone(),
                subscribers: subscribers.clone(),
                epochs: epochs.clone(),
                grants: grants.clone(),
            });
            let card = Arc::new(StaticAgentCard::new(card(
                &format!("{}{prefix}/rpc", base.as_str().trim_end_matches('/')),
                title,
                description,
            )));
            let export_router = Router::new()
                .merge(agent_card_router(card).layer(middleware::map_response(private_card)))
                .nest_service(
                    "/rpc",
                    jsonrpc_router(handler).layer(middleware::from_fn(exactly_one)),
                )
                .layer(RequestBodyLimitLayer::new(BODY_LIMIT));
            router = router.nest(&prefix, export_router);
        }
        // 186 (D5): the contact-only return route: answers in conversations we opened. No
        // card; the gate admits only a contact's own `theuth/a2a/1` stream to it.
        if !contacts.is_empty() {
            let handler = Arc::new(Handler {
                export: RETURN_ROUTE.to_owned(),
                export_id: None,
                target: Target::Return,
                agents: agents.clone(),
                subscribers: subscribers.clone(),
                epochs: epochs.clone(),
                grants: grants.clone(),
            });
            router = router.nest(
                &format!("/agents/{RETURN_ROUTE}"),
                Router::new()
                    .nest_service(
                        "/rpc",
                        jsonrpc_router(handler).layer(middleware::from_fn(exactly_one)),
                    )
                    .layer(RequestBodyLimitLayer::new(BODY_LIMIT)),
            );
            // 200 (D3): the one `~session` handler every contact's `session:<uuid>` path is
            // rewritten to; never reached by its own name.
            let handler = Arc::new(Handler {
                export: SESSION_ROUTE.to_owned(),
                export_id: None,
                target: Target::Session,
                agents: agents.clone(),
                subscribers: subscribers.clone(),
                epochs: epochs.clone(),
                grants: grants.clone(),
            });
            router = router.nest(
                &format!("/agents/{SESSION_ROUTE}"),
                Router::new()
                    .nest_service(
                        "/rpc",
                        jsonrpc_router(handler).layer(middleware::from_fn(exactly_one)),
                    )
                    .layer(RequestBodyLimitLayer::new(BODY_LIMIT)),
            );
        }
        Self {
            router,
            principals,
            contacts,
            exports: staged
                .exports
                .values()
                .map(|e| (e.name.clone(), e.session))
                .collect(),
            agents: staged.agents.keys().cloned().collect(),
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

    /// 183 F1 (review 1): admit one contact-only request on `theuth/a2a/1` (its card, the
    /// offers notice) from the contact pinned to `eid`, under the same per-principal rate and
    /// concurrency bounds `dispatch` applies to the A2A routes.
    pub(crate) fn admit_contact(&self, eid: &str) -> Result<OwnedSemaphorePermit, StatusCode> {
        let p = self.contacts.get(eid).ok_or(StatusCode::UNAUTHORIZED)?;
        if !p.admit_rate() {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        p.concurrent
            .clone()
            .try_acquire_owned()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
    }

    /// 183 §7.7: the export a contact's `POST /rpc` reaches: the first one granted to the
    /// contact pinned to `eid` (as [`dispatch`] maps it).
    pub(crate) fn first_export(&self, eid: &str) -> Option<String> {
        self.contacts.get(eid)?.exports.iter().min().cloned()
    }

    /// 200 (D7): the grant epoch of the contact pinned to `eid` and whether it has full
    /// access, from this one configuration (the listing runs inside that epoch).
    pub(crate) fn full_access_of(&self, eid: &str) -> Option<(CancellationToken, bool)> {
        let p = self.contacts.get(eid)?;
        let on = self
            .staged
            .peers
            .get(&p.peer)
            .is_some_and(|x| x.full_access);
        Some((p.epoch.clone(), on))
    }

    /// The export exists and its session is effectively enabled right now; an agent export
    /// while agent communication is on (its inbox is checked at admission).
    fn open(&self, export: &str) -> bool {
        let Some(policy) = self.policy.as_ref() else {
            return false;
        };
        if self.agents.contains(export) {
            return policy.0.read().global();
        }
        let Some(session) = self.exports.get(export) else {
            return false;
        };
        policy.0.read().enabled(*session).is_ok()
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
    // 200 (D3): so is any inbound copy of the session header.
    crate::contacts::sessions::strip_session_header(req.headers_mut());
    let mut export = req
        .uri()
        .path()
        .strip_prefix("/agents/")
        .and_then(|rest| rest.split('/').next())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // 183 §7.2: a `theuth/a2a/1` stream's caller is its pinned endpoint (the reach put it
    // on the request); everyone else presents a bearer.
    let principal = match req.extensions().get::<ContactCaller>() {
        Some(ContactCaller(eid)) => live.contacts.get(eid).cloned(),
        None => req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .and_then(|b| live.authenticate(b))
            .cloned(),
    };
    // Credentials are checked before existence: an unauthenticated caller
    // learns nothing about which exports exist (§6.1).
    let Some(principal) = principal else {
        return unauthorized();
    };
    // 183 §7.7: a contact that names no export (`POST /rpc`) reaches the first one granted
    // to it: `peer:<name>` addresses the contact, not one of its exports.
    if export.is_none() && req.extensions().get::<ContactCaller>().is_some() {
        let first = principal.exports.iter().min().cloned();
        if let Some(e) = &first
            && let Ok(uri) = format!("/agents/{e}{}", req.uri().path()).parse()
        {
            *req.uri_mut() = uri;
        }
        export = first;
    }
    // Granted, existing and effectively enabled right now, or not found: a
    // disabled export is not disclosed and its cached routes never answer. 186 (D5): the
    // return route is every contact's, on its own stream only.
    let contact = req.extensions().get::<ContactCaller>().is_some();
    // 200 (D3): a contact's `session:<uuid>` is the one `~session` handler, the session in the
    // trusted header; `~session` by name, or a bearer on a `session:` path, is not found.
    let mut session = false;
    if let Some(e) = export.as_deref() {
        if e == SESSION_ROUTE {
            return StatusCode::NOT_FOUND.into_response();
        }
        if let Some(id) = session_route(e) {
            if !contact {
                return StatusCode::NOT_FOUND.into_response();
            }
            let rest = &req.uri().path()[format!("/agents/{e}").len()..];
            let Ok(uri) = format!("/agents/{SESSION_ROUTE}{rest}").parse() else {
                return StatusCode::NOT_FOUND.into_response();
            };
            *req.uri_mut() = uri;
            let Ok(v) = HeaderValue::from_str(&id.to_string()) else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            req.headers_mut()
                .insert(crate::contacts::sessions::SESSION_HEADER, v);
            session = true;
        }
    }
    let Some(export) = export.filter(|e| {
        (contact && (session || e == RETURN_ROUTE))
            || (principal.exports.contains(e) && live.open(e))
    }) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let _permit = if req.extensions().get::<Charged>().is_some() {
        None
    } else {
        if !principal.admit_rate() {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        let Ok(permit) = principal.concurrent.clone().try_acquire_owned() else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, HeaderValue::from_static("1"))],
            )
                .into_response();
        };
        Some(permit)
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
        version: platform::version::get().to_owned(),
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

/// [`check_version`] of a request by its headers (191: the receiving machine checks it before
/// it forwards a call the owner then serves as this version).
pub(crate) fn check_version_of(headers: &axum::http::HeaderMap) -> Result<(), A2AError> {
    let v = headers
        .get(VERSION_PARAM)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    check_version(&ServiceParams::from([(
        VERSION_PARAM.to_owned(),
        v.into_iter().collect(),
    )]))
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
/// `message` to an empty one, so emptiness is checked here; text parts, and 199 (D1) file
/// references from `caller` (the contact it came from; `None` for an HTTP peer, which
/// sends none) under `extensions == [EXT_FILES]`; no push config, no other extensions.
/// 199-E3 (P3, D16): an HTTP peer's files, inline `raw` (within [`BODY_LIMIT`]) or at an
/// `https` URL without credentials (what it serves is checked when it is fetched).
/// The one rule decoding loses (exactly one content field per part) is [`exactly_one`]'s.
pub fn validate_send(
    req: &SendMessageRequest,
    caller: Option<&crate::config::ContactFacts>,
) -> Result<(), A2AError> {
    use crate::contacts::{EXT_FILES, files};
    let m = &req.message;
    if m.message_id.trim().is_empty() {
        return Err(A2AError::invalid_params("message.messageId is required"));
    }
    if m.parts.is_empty() {
        return Err(A2AError::invalid_params("message.parts must not be empty"));
    }
    let with_files = match m.extensions.as_deref() {
        None | Some([]) => false,
        Some([e]) if e == EXT_FILES => true,
        Some(_) => {
            return Err(A2AError::new(
                error_code::EXTENSION_SUPPORT_REQUIRED,
                "extensions are not supported",
            ));
        }
    };
    let members = caller
        .filter(|f| !f.id.is_empty())
        .map_or(&[][..], |f| &f.eids[..]);
    for (i, p) in m.parts.iter().enumerate() {
        let invalid = |why: &str| A2AError::invalid_params(format!("message.parts[{i}]: {why}"));
        match &p.content {
            PartContent::Text(_) => {}
            PartContent::Url(url) if with_files => {
                files::check_reference(url, p.metadata.as_ref(), members)
                    .map_err(|why| invalid(&why))?;
            }
            PartContent::Raw(_) if caller.is_none() => {}
            PartContent::Url(url) if caller.is_none() => {
                agent::communication::files::foreign::https_url(url).map_err(invalid)?;
            }
            _ => return Err(A2AError::content_type_not_supported()),
        }
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

/// 199 (D1): the body layer of every JSON-RPC route, inside [`BODY_LIMIT`]:
/// `contacts::files::check_part_content` on the raw JSON before the SDK decodes it (which
/// takes the first content field it knows, so a part with both `text` and `url` would pass
/// as text). A refusal is the JSON-RPC `invalid_params` naming the part; a body that is no
/// JSON object is the router's to refuse.
async fn exactly_one(req: Request, next: middleware::Next) -> Response {
    let (head, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, BODY_LIMIT).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Err(why) = crate::contacts::files::check_part_content(&v)
    {
        let e = A2AError::invalid_params(why);
        return axum::Json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": v.get("id").cloned().unwrap_or_default(),
            "error": {"code": e.code, "message": e.message},
        }))
        .into_response();
    }
    next.run(Request::from_parts(head, axum::body::Body::from(bytes)))
        .await
}

/// What one export route publishes.
#[derive(Clone, Debug)]
enum Target {
    /// One exact session (an HTTP peer's export, or an exported chat, D9).
    Chat(SessionId),
    /// 183 §7.6: an agent: each contact gets its own inbox. 191 (D4): and its home, the only
    /// machine that opens one.
    Agent(String, agent::communication::network::InboxHome),
    /// 186 (D5): the return route: a contact's answers in conversations this engine opened.
    Return,
    /// 200 (D3): the one `~session` handler: the session a full-access contact named, from
    /// the trusted header `dispatch` set; `SendMessage`, `GetTask` and `CancelTask` only.
    Session,
}

/// Application seam per export: typed requests → agent commands (§4).
struct Handler {
    export: String,
    /// 191 (D2): its replicated id (the inbox marker names it).
    export_id: Option<String>,
    target: Target,
    agents: ActorRef<Agents>,
    subscribers: Arc<Semaphore>,
    /// Binding → grant epoch of the configuration these routes belong to.
    epochs: Arc<HashMap<Uuid, CancellationToken>>,
    /// 183 §7.6: contact binding → its alias and facts (machine, first contact).
    grants: Arc<HashMap<Uuid, ContactGrant>>,
}

fn revoked() -> A2AError {
    A2AError::unsupported_operation("export unavailable: peer grant revoked")
}

impl Handler {
    /// Version, the verified binding and its live grant epoch.
    fn caller(&self, params: &ServiceParams) -> Result<(Uuid, &CancellationToken), A2AError> {
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
        Ok((binding, route))
    }

    /// `send`: a `SendMessage` or the streaming send (191 S4: a read never opens an inbox).
    async fn admit(&self, params: &ServiceParams, send: bool) -> Result<A2aPrincipal, A2AError> {
        if matches!(self.target, Target::Session) {
            return self.admit_session(params).await;
        }
        let (binding, route) = self.caller(params)?;
        // 183 §7.6: a contact's origin rides with every admission (the first-contact hold).
        // 190 (D1): every share is full.
        let grant = self.grants.get(&binding);
        let contact = grant.map(|g| agent::communication::ContactOrigin {
            contact: g.alias.clone(),
            machine: g.facts.machine.clone(),
            export: self.export.clone(),
            mode: agent::ShareMode::Full,
        });
        let ask_first = match grant.map(|g| g.facts.first_contact.get(&self.export)) {
            None | Some(Some(crate::config::FirstContact::Allow)) => false,
            Some(Some(crate::config::FirstContact::Block)) => {
                return Err(A2AError::unsupported_operation(format!(
                    "export unavailable: {} is not allowed to reach {}",
                    grant.map(|g| g.alias.as_str()).unwrap_or_default(),
                    self.export
                )));
            }
            Some(_) => true,
        };
        let session = match (&self.target, &contact) {
            (Target::Chat(s), _) => *s,
            (Target::Agent(agent, home), Some(origin)) => {
                self.ask(agent::communication::network::ContactInbox {
                    binding,
                    agent: agent.clone(),
                    origin: origin.clone(),
                    export_id: self.export_id.clone(),
                    home: home.clone(),
                    send,
                })
                .await?
            }
            (Target::Agent(..), None) => {
                return Err(A2AError::unsupported_operation(
                    "export unavailable: only contacts reach agent exports",
                ));
            }
            (Target::Return, _) => {
                return Err(A2AError::unsupported_operation(
                    "the return route takes answers (SendMessage) only",
                ));
            }
            (Target::Session, _) => return Err(A2AError::internal("admit_session serves it")),
        };
        Ok(A2aPrincipal {
            binding,
            export: self.export.clone(),
            session,
            route: route.clone(),
            contact,
            ask_first,
        })
    }

    /// 200 (D3, D4 (1)-(3)): a contact's `session:<uuid>` (the gate ran): full access, else
    /// `full access is off`; the row half on this machine, else the plain `not owner`. The
    /// principal's export is `session:<uuid>` (never the mounted name); no first-contact hold.
    async fn admit_session(&self, params: &ServiceParams) -> Result<A2aPrincipal, A2AError> {
        use agent::communication::network::access;
        let (binding, route) = self.caller(params)?;
        let session = params
            .get(crate::contacts::sessions::SESSION_HEADER)
            .and_then(|v| v.first())
            .and_then(|v| v.parse::<SessionId>().ok())
            .ok_or_else(|| A2AError::internal("session layer did not run"))?;
        let grant = self.grants.get(&binding).ok_or_else(|| {
            A2AError::unsupported_operation("export unavailable: only contacts reach sessions")
        })?;
        if !grant.full_access {
            return Err(A2AError::unsupported_operation(access::OFF));
        }
        self.ask(access::FullAccessRow(session)).await?;
        let export = format!("{}{session}", access::SESSION_PREFIX);
        Ok(A2aPrincipal {
            binding,
            export: export.clone(),
            session,
            route: route.clone(),
            contact: Some(agent::communication::ContactOrigin {
                contact: grant.alias.clone(),
                machine: grant.facts.machine.clone(),
                export,
                mode: agent::ShareMode::Full,
            }),
            ask_first: false,
        })
    }

    /// 200 (D3): a method `~session` does not serve.
    fn session_only(&self) -> Result<(), A2AError> {
        if matches!(self.target, Target::Session) {
            return Err(A2AError::unsupported_operation(
                "a contact's session takes SendMessage, GetTask and CancelTask only",
            ));
        }
        Ok(())
    }

    /// 191 (D3, S4): the strict session resolve of a contact's message, before any task
    /// exists: served only by the machine that owns the session (its row here, its owner
    /// this engine), else the A2A error `not owner` (`NOT_OWNER_CODE`), and the contact's
    /// client tries its next roster member.
    async fn owned(&self, p: &A2aPrincipal) -> Result<(), A2AError> {
        if p.contact.is_some() {
            self.ask(agent::communication::network::ContactOwned(p.session))
                .await?;
        }
        Ok(())
    }

    /// 199: the contact facts of `p`'s binding (its machines name the files it may offer).
    fn facts(&self, p: &A2aPrincipal) -> Option<&crate::config::ContactFacts> {
        self.grants.get(&p.binding).map(|g| &g.facts)
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
        // 186 (D5): an answer on the return route, from a contact (the gate let only its own
        // stream here).
        if matches!(self.target, Target::Return) {
            let (binding, route) = self.caller(params)?;
            if !self.grants.contains_key(&binding) {
                return Err(A2AError::task_not_found("conversation"));
            }
            validate_send(&req, self.grants.get(&binding).map(|g| &g.facts))?;
            return self
                .ask(A2aReturn {
                    binding,
                    route: route.clone(),
                    request: req,
                })
                .await;
        }
        let principal = self.admit(params, true).await?;
        self.owned(&principal).await?;
        validate_send(&req, self.facts(&principal))?;
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
        self.session_only()?;
        let principal = self.admit(params, true).await?;
        self.owned(&principal).await?;
        validate_send(&req, self.facts(&principal))?;
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
        let principal = self.admit(params, false).await?;
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
        self.session_only()?;
        let principal = self.admit(params, false).await?;
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
        let principal = self.admit(params, false).await?;
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
        self.session_only()?;
        let principal = self.admit(params, false).await?;
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
        self.admit(params, false).await?;
        Err(A2AError::push_notification_not_supported())
    }

    async fn get_push_config(
        &self,
        params: &ServiceParams,
        _req: GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        self.admit(params, false).await?;
        Err(A2AError::push_notification_not_supported())
    }

    async fn list_push_configs(
        &self,
        params: &ServiceParams,
        _req: ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, A2AError> {
        self.admit(params, false).await?;
        Err(A2AError::push_notification_not_supported())
    }

    async fn delete_push_config(
        &self,
        params: &ServiceParams,
        _req: DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2AError> {
        self.admit(params, false).await?;
        Err(A2AError::push_notification_not_supported())
    }

    async fn get_extended_agent_card(
        &self,
        params: &ServiceParams,
        _req: GetExtendedAgentCardRequest,
    ) -> Result<AgentCard, A2AError> {
        self.admit(params, false).await?;
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

/// 183 §7.2: what the `theuth/a2a/1` gate decides for one inbound connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrohGate {
    /// A live, unblocked contact: each stream's HTTP request enters [`app`] with this binding
    /// as the trusted principal (the bearer step is replaced; export, rate, concurrency and
    /// body limits stay).
    Contact {
        /// Its `peer_bindings.id`.
        binding: Uuid,
        /// Its alias.
        alias: String,
    },
    /// A stranger while an Add contact window is open: only a `{"pair": …}` ceremony.
    Pair,
    /// Anyone else: a stranger outside a window, a blocked or removed contact, a family peer.
    Refuse,
}

/// The gate for the remote endpoint id `remote` (family peers never get here: the mesh
/// refuses them first). A live contact is its binding; a blocked one is refused even inside
/// a window; a stranger passes only for the ceremony while a window is open. 191 (D2): an
/// endpoint in the deny set (a quarantined or blocked record's) is refused before anything
/// else; a removed record's endpoint is never admitted as a contact (it may pair again in a
/// window), and nothing is admitted before `contacts_ready`, a ceremony included (the deny
/// set is whole only after the first pass).
pub fn iroh_gate(live: &Live, remote: &str, window_open: bool, ready: bool) -> IrohGate {
    if live.staged.deny.contains(remote) {
        return IrohGate::Refuse;
    }
    match live.staged.contact_by_eid(remote) {
        Some(alias) => match (
            ready && !live.staged.removed.contains(remote),
            live.staged.contacts[alias].blocked,
            live.bindings.get(alias),
        ) {
            (true, false, Some(binding)) => IrohGate::Contact {
                binding: *binding,
                alias: alias.to_owned(),
            },
            _ => IrohGate::Refuse,
        },
        None if window_open && ready => IrohGate::Pair,
        None => IrohGate::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 183 review 1, 2: a contact-only request (its card, the offers notice) is bounded like
    /// the A2A routes: an unknown caller 401, every permit taken 503, a spent bucket 429.
    #[test]
    fn contact_only_requests_share_the_contacts_bounds() {
        let bob = || {
            Arc::new(Principal::new(
                Uuid::from_u128(1),
                "bob".into(),
                HashSet::new(),
                CancellationToken::new(),
            ))
        };
        let mut live = Live::empty();
        live.contacts.insert("bob-eid".into(), bob());
        assert_eq!(
            live.admit_contact("nobody").unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        let held: Vec<_> = (0..CONCURRENT_PER_PEER)
            .map(|_| live.admit_contact("bob-eid").unwrap())
            .collect();
        assert_eq!(
            live.admit_contact("bob-eid").unwrap_err(),
            StatusCode::SERVICE_UNAVAILABLE,
            "every permit taken"
        );
        drop(held);
        assert!(live.admit_contact("bob-eid").is_ok(), "permits come back");

        // A fresh bucket: its burst, one at a time, then 429.
        live.contacts.insert("bob-eid".into(), bob());
        for _ in 0..RATE_PER_SEC as usize {
            drop(live.admit_contact("bob-eid").unwrap());
        }
        assert_eq!(
            live.admit_contact("bob-eid").unwrap_err(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

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
            contacts: HashMap::new(),
            exports: exports.iter().map(|(e, s)| ((*e).to_owned(), *s)).collect(),
            agents: HashSet::new(),
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

    /// 200 (D3): only a contact's `session:<uuid>` path reaches the one `~session` handler,
    /// with the session in the trusted header `dispatch` set; `~session` by name, a bearer on
    /// a `session:` path and a non-canonical id are 404; a forged header on a share path is
    /// stripped, never seen.
    #[tokio::test]
    async fn only_a_contacts_session_path_reaches_the_session_handler() {
        use crate::contacts::sessions::SESSION_HEADER;
        use agent::communication::{PeerScope, Policy};
        use axum::http::HeaderMap;
        use axum::routing::post;
        async fn echo(headers: HeaderMap) -> Response {
            let seen = headers
                .get(SESSION_HEADER)
                .cloned()
                .unwrap_or_else(|| HeaderValue::from_static("none"));
            (StatusCode::OK, [("x-seen-session", seen)]).into_response()
        }
        let research = SessionId::new();
        let mut policy = Policy::new(true, "test");
        policy.record_root(research, PeerScope::Owner).unwrap();
        let router = Router::new()
            .nest(
                "/agents/research",
                Router::new().nest_service("/rpc", post(echo)),
            )
            .nest(
                &format!("/agents/{SESSION_ROUTE}"),
                Router::new().nest_service("/rpc", post(echo)),
            );
        let principal = |id: u128| {
            Arc::new(Principal::new(
                Uuid::from_u128(id),
                "maya".into(),
                HashSet::from(["research".to_owned()]),
                CancellationToken::new(),
            ))
        };
        let shared: Shared = Arc::new(RwLock::new(Arc::new(Live {
            router,
            principals: vec![(Token::for_test("tok-a"), principal(1))],
            contacts: HashMap::from([("maya-eid".to_owned(), principal(2))]),
            exports: HashMap::from([("research".to_owned(), research)]),
            agents: HashSet::new(),
            policy: Some(SharedPolicy::new(policy)),
            staged: Staged::default(),
            bindings: BTreeMap::new(),
        })));
        let id = SessionId::new();
        let send = |path: String, contact: bool, forged: bool| {
            let shared = shared.clone();
            async move {
                let mut req = Request::builder().method("POST").uri(path);
                if !contact {
                    req = req.header(header::AUTHORIZATION, "Bearer tok-a");
                }
                if forged {
                    req = req.header(SESSION_HEADER, SessionId::new().to_string());
                }
                let mut req = req.body(axum::body::Body::empty()).unwrap();
                if contact {
                    req.extensions_mut()
                        .insert(ContactCaller("maya-eid".into()));
                }
                app(shared).oneshot(req).await.unwrap()
            }
        };
        let r = send(format!("/agents/session:{id}/rpc"), true, true).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["x-seen-session"], id.to_string().as_str());
        let r = send("/agents/research/rpc".into(), true, true).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["x-seen-session"], "none", "a forged header");
        for (path, contact) in [
            (format!("/agents/{SESSION_ROUTE}/rpc"), true),
            (format!("/agents/session:{id}/rpc"), false),
            (
                format!("/agents/session:{}/rpc", id.to_string().to_uppercase()),
                true,
            ),
        ] {
            assert_eq!(
                send(path.clone(), contact, false).await.status(),
                StatusCode::NOT_FOUND,
                "{path} (contact: {contact})"
            );
        }
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
        assert!(validate_send(&send(ok.clone()), None).is_ok());

        let mut empty_id = ok.clone();
        empty_id.message_id.clear();
        assert_eq!(
            validate_send(&send(empty_id), None).unwrap_err().code,
            error_code::INVALID_PARAMS
        );

        let no_parts = Message::new(Role::User, vec![]);
        assert_eq!(
            validate_send(&send(no_parts), None).unwrap_err().code,
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
        // 199-E3: an HTTP peer's URL part must be a credential-free `https` file URL.
        assert_eq!(
            validate_send(&send(file), None).unwrap_err().code,
            error_code::INVALID_PARAMS
        );

        let mut ext = ok.clone();
        ext.extensions = Some(vec!["https://example/ext".into()]);
        assert_eq!(
            validate_send(&send(ext), None).unwrap_err().code,
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
            validate_send(&push, None).unwrap_err().code,
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
            validate_send(&modes, None).unwrap_err().code,
            error_code::CONTENT_TYPE_NOT_SUPPORTED
        );
    }

    /// 199-E3 (P3, D16): from an HTTP peer (no files extension), inline `raw` parts and
    /// `https` URL parts are admitted beside text; any other URL, or one with userinfo, is
    /// `invalid_params` naming the part; `data` stays unsupported. A contact (a theuth)
    /// sends no such parts.
    #[test]
    fn foreign_files_are_admitted_inline_or_at_https_urls_only() {
        let msg = |parts: Vec<Part>| {
            let mut m = Message::new(Role::User, parts);
            m.message_id = "m1".into();
            send(m)
        };
        let raw = Part::raw(vec![0x89, b'P', b'N', b'G'])
            .with_filename("dot.png")
            .with_media_type("image/png");
        let https = Part::url("https://files.example/report.pdf");
        assert!(
            validate_send(
                &msg(vec![Part::text("see"), raw.clone(), https.clone()]),
                None
            )
            .is_ok()
        );
        assert!(validate_send(&msg(vec![raw.clone()]), None).is_ok());
        for (url, why) in [
            ("http://files.example/report.pdf", "the url is not https"),
            (
                "https://user:pw@files.example/f",
                "the url carries credentials",
            ),
            (
                "https://user@files.example/f",
                "the url carries credentials",
            ),
            ("file:///etc/passwd", "the url is not https"),
            ("not a url", "the url does not parse"),
        ] {
            let e = validate_send(&msg(vec![Part::text("see"), Part::url(url)]), None).unwrap_err();
            assert_eq!(e.code, error_code::INVALID_PARAMS, "{url}");
            assert_eq!(e.message, format!("message.parts[1]: {why}"), "{url}");
        }
        assert_eq!(
            validate_send(&msg(vec![Part::data(serde_json::json!({"k": 1}))]), None)
                .unwrap_err()
                .code,
            error_code::CONTENT_TYPE_NOT_SUPPORTED
        );
        let maya = contact("rec-maya", &["7a7a"]);
        for p in [raw, https] {
            assert_eq!(
                validate_send(&msg(vec![p]), Some(&maya)).unwrap_err().code,
                error_code::CONTENT_TYPE_NOT_SUPPORTED
            );
        }
    }

    fn contact(id: &str, eids: &[&str]) -> crate::config::ContactFacts {
        crate::config::ContactFacts {
            id: id.into(),
            eid: eids[0].into(),
            eids: eids.iter().map(|e| (*e).to_owned()).collect(),
            display: String::new(),
            machine: String::new(),
            first_contact: Default::default(),
            blocked: false,
            paired_ms: 0,
            addrs: Vec::new(),
        }
    }

    /// 199 (D1): a file-bearing message built as the client builds it and carried as
    /// ProtoJSON both ways is admitted only from a machine of the sending contact, only
    /// under `extensions: [EXT_FILES]` and only with sound metadata; plain text needs no
    /// extension, exactly as before files.
    #[test]
    fn file_references_are_admitted_from_the_senders_machines_only() {
        use crate::contacts::{EXT_FILES, files};
        use agent::communication::files::{ContactFile, FileState, OutState};
        let holder = "5b2f0c8e9d41a7b3c6e05f1d2a9b8c7e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b9c";
        let file = ContactFile {
            id: "0f8e2d4c6a1b3957e0d2c4b6a8f01357".into(),
            holder: holder.into(),
            state: FileState::Out(OutState::Queued),
            exchange: Uuid::nil(),
            contact_id: "rec-maya".into(),
            contact: "maya".into(),
            name: "handoff.pdf".into(),
            media_type: "application/pdf".into(),
            size: Some(2_516_582),
            blake3: Some("a3f1c09be27d4458e61f0b9c2d7e8a4f5b6c3d2e1f0a9b8c7d6e5f4a3b2c1d0e".into()),
            done: 0,
            error: None,
            path: None,
            machine: "xeus".into(),
            deadline: "2026-11-24T14:32:00.75Z".parse().unwrap(),
        };
        let mut m = Message::new(
            Role::User,
            vec![Part::text("see"), files::reference_part(&file)],
        );
        m.message_id = "m1".into();
        m.extensions = Some(vec![EXT_FILES.to_owned()]);
        let wire = a2a_pb::protojson_conv::to_value(&send(m)).unwrap();
        let part = &wire["message"]["parts"][1];
        assert_eq!(part["url"], files::file_url(holder, &file.id).as_str());
        assert_eq!(
            part["metadata"][EXT_FILES]["expires"],
            "2026-11-24T14:32:00Z"
        );
        assert_eq!(wire["message"]["extensions"][0], EXT_FILES);
        let back: SendMessageRequest = a2a_pb::protojson_conv::from_value(wire).unwrap();

        let maya = contact("rec-maya", &["7a7a", holder]);
        assert!(validate_send(&back, Some(&maya)).is_ok());
        let invalid = |r: &SendMessageRequest, caller: Option<&crate::config::ContactFacts>| {
            let e = validate_send(r, caller).unwrap_err();
            assert_eq!(e.code, error_code::INVALID_PARAMS, "{}", e.message);
            assert!(e.message.starts_with("message.parts[1]: "), "{}", e.message);
        };
        // Another contact's machine, an import without a record, an HTTP peer.
        invalid(&back, Some(&contact("rec-bob", &["7a7a"])));
        invalid(&back, Some(&contact("", &[holder])));
        invalid(&back, None);
        let with = |f: &dyn Fn(&mut Part)| {
            let mut r = back.clone();
            f(&mut r.message.parts[1]);
            r
        };
        let meta = |p: &mut Part| {
            p.metadata
                .as_mut()
                .unwrap()
                .get_mut(EXT_FILES)
                .unwrap()
                .clone()
        };
        for broken in [
            with(&|p| {
                p.content =
                    PartContent::Url(format!("https://{holder}/a2a/files/{}", "0".repeat(32)))
            }),
            with(&|p| p.metadata = None),
            with(&|p| {
                let mut v = meta(p);
                v["size"] = serde_json::json!(5_000_000_001u64);
                p.metadata = Some(HashMap::from([(EXT_FILES.to_owned(), v)]));
            }),
            with(&|p| {
                let mut v = meta(p);
                v["expires"] = serde_json::json!("next week");
                p.metadata = Some(HashMap::from([(EXT_FILES.to_owned(), v)]));
            }),
        ] {
            invalid(&broken, Some(&maya));
        }
        // No extension: the part is unsupported, as on an engine before files; another
        // extension beside it is still required.
        let mut bare = back.clone();
        bare.message.extensions = None;
        assert_eq!(
            validate_send(&bare, Some(&maya)).unwrap_err().code,
            error_code::CONTENT_TYPE_NOT_SUPPORTED
        );
        let mut more = back.clone();
        more.message.extensions = Some(vec![EXT_FILES.to_owned(), "https://example/ext".into()]);
        assert_eq!(
            validate_send(&more, Some(&maya)).unwrap_err().code,
            error_code::EXTENSION_SUPPORT_REQUIRED
        );
        // Plain text without extensions, from a contact or not.
        let mut text = Message::new(Role::User, vec![Part::text("hi")]);
        text.message_id = "m2".into();
        assert!(validate_send(&send(text.clone()), Some(&maya)).is_ok());
        assert!(validate_send(&send(text), None).is_ok());
    }

    /// 199 (D1): a part with both `text` and `url` is refused on the raw JSON-RPC body (the
    /// SDK would take it as text), naming the part with the request's id; every other body
    /// passes on unchanged.
    #[tokio::test]
    async fn the_body_layer_refuses_a_part_with_two_contents() {
        let app = Router::new()
            .route(
                "/",
                axum::routing::post(|b: axum::body::Bytes| async move { b }),
            )
            .layer(middleware::from_fn(exactly_one));
        let call = |body: serde_json::Value| {
            let app = app.clone();
            async move {
                let req = axum::http::Request::post("/")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap();
                let resp = app.oneshot(req).await.unwrap();
                let b = axum::body::to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap();
                serde_json::from_slice::<serde_json::Value>(&b).unwrap()
            }
        };
        let body = |part: serde_json::Value| {
            serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "SendMessage",
                "params": {"message": {"messageId": "m", "role": "ROLE_USER", "parts": [part]}}})
        };
        let got = call(body(
            serde_json::json!({"text": "x", "url": "iroh://h/a2a/files/0"}),
        ))
        .await;
        assert_eq!(got["id"], 7);
        assert_eq!(got["error"]["code"], error_code::INVALID_PARAMS);
        assert_eq!(
            got["error"]["message"],
            "message.parts[0] must set exactly one of text, raw, url or data"
        );
        let one = body(serde_json::json!({"text": "x"}));
        assert_eq!(call(one.clone()).await, one);
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

    /// The reach's handler over a fixed [`Live`]: the gate and the HTTP seam as
    /// `contacts::reach` runs them, without an engine behind the router.
    struct TestGate(Shared, bool);

    impl mesh::net::ContactAlpn for TestGate {
        fn admit(&self, remote: &str) -> bool {
            iroh_gate(&self.0.read(), remote, self.1, true) != IrohGate::Refuse
        }

        fn stream(
            &self,
            remote: String,
            mut stream: Box<dyn replica::ByteStream>,
        ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
            let shared = self.0.clone();
            Box::pin(async move {
                let Ok(line) = crate::contacts::reach::read_line(&mut stream).await else {
                    return;
                };
                if matches!(
                    iroh_gate(&shared.read(), &remote, false, true),
                    IrohGate::Contact { .. }
                ) {
                    let io = crate::contacts::reach::Prefixed::new(line, stream);
                    let _ = crate::contacts::reach::serve_http(shared, remote, Box::new(io), None)
                        .await;
                }
            })
        }
    }

    /// One `POST /rpc` over `theuth/a2a/1` from `from` to `to`, with a forged seam header:
    /// the status and the principal the export router saw.
    async fn rpc_over_iroh(
        from: &mesh::net::Net,
        to: &mesh::net::Net,
    ) -> Result<(StatusCode, String), String> {
        use http_body_util::Full;
        let addrs: Vec<std::net::SocketAddr> = to
            .a2a_reach()
            .0
            .iter()
            .filter_map(|a| a.parse().ok())
            .collect();
        let stream = from
            .a2a_stream(&to.id.0, &addrs, None)
            .await
            .map_err(|e| e.to_string())?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                .await
                .map_err(|e| e.to_string())?;
        tokio::spawn(conn);
        let req = hyper::Request::post("/rpc")
            .header(hyper::header::HOST, "theuth")
            .header(PRINCIPAL_HEADER, "00000000-0000-0000-0000-00000000dead")
            .body(Full::new(bytes::Bytes::new()))
            .unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(10), sender.send_request(req))
            .await
            .map_err(|_| "no answer".to_owned())?
            .map_err(|e| e.to_string())?;
        let seen = resp
            .headers()
            .get("x-seen-principal")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        Ok((resp.status(), seen))
    }

    /// 183 §7.2: on `theuth/a2a/1` a pinned contact's request reaches its export (no bearer:
    /// its binding is the trusted seam, a forged header is replaced, `POST /rpc` addresses
    /// its first granted export); a stranger is refused, inside a window too (only the
    /// ceremony passes there), and so is a blocked contact.
    #[tokio::test]
    async fn the_contact_gate_admits_a_pinned_contact_and_refuses_strangers_and_blocked_ones() {
        use agent::communication::{PeerScope, Policy};
        use axum::http::HeaderMap;
        use axum::routing::post;
        async fn echo(headers: HeaderMap) -> Response {
            let seen = headers
                .get(PRINCIPAL_HEADER)
                .cloned()
                .unwrap_or_else(|| HeaderValue::from_static("none"));
            (StatusCode::OK, [("x-seen-principal", seen)]).into_response()
        }
        let dirs: Vec<tempfile::TempDir> = (0..5).map(|_| tempfile::tempdir().unwrap()).collect();
        let a = crate::contacts::e2e_tests::node("a", dirs[0].path()).await;
        let b = crate::contacts::e2e_tests::node("b", dirs[1].path()).await;
        let c = crate::contacts::e2e_tests::node("c", dirs[2].path()).await;
        let research = SessionId::new();
        let mut policy = Policy::new(true, "test");
        policy.record_root(research, PeerScope::Owner).unwrap();
        let policy = SharedPolicy::new(policy);
        let binding = Uuid::from_u128(7);
        let live = |blocked: bool, window: bool| {
            let mut staged = Staged::default();
            staged.contacts.insert(
                "bob".into(),
                crate::config::ContactFacts {
                    id: String::new(),
                    eid: b.id.0.clone(),
                    eids: vec![b.id.0.clone()],
                    display: "bob".into(),
                    machine: "Bob's MacBook".into(),
                    first_contact: BTreeMap::new(),
                    blocked,
                    paired_ms: 1,
                    addrs: Vec::new(),
                },
            );
            let shared: Shared = Arc::new(RwLock::new(Arc::new(Live {
                router: Router::new().nest(
                    "/agents/research",
                    Router::new().nest_service("/rpc", post(echo)),
                ),
                principals: Vec::new(),
                contacts: HashMap::from([(
                    b.id.0.clone(),
                    Arc::new(Principal::new(
                        binding,
                        "bob".into(),
                        HashSet::from(["research".to_owned()]),
                        CancellationToken::new(),
                    )),
                )]),
                exports: HashMap::from([("research".to_owned(), research)]),
                agents: HashSet::new(),
                policy: Some(policy.clone()),
                staged,
                bindings: BTreeMap::from([("bob".to_owned(), binding)]),
            })));
            Arc::new(TestGate(shared, window))
        };
        // An endpoint takes one handler: one Net per gate state.
        a.set_contacts(live(false, false));
        let (status, seen) = rpc_over_iroh(&b, &a).await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            seen,
            binding.to_string(),
            "the contact's binding, not the forged one"
        );
        assert!(
            rpc_over_iroh(&c, &a).await.is_err(),
            "a stranger is refused"
        );

        let open = crate::contacts::e2e_tests::node("a-window", dirs[3].path()).await;
        open.set_contacts(live(false, true));
        assert!(
            rpc_over_iroh(&c, &open).await.is_err(),
            "inside a window a stranger gets only the ceremony"
        );
        let blocked = crate::contacts::e2e_tests::node("a-blocked", dirs[4].path()).await;
        blocked.set_contacts(live(true, true));
        assert!(
            rpc_over_iroh(&b, &blocked).await.is_err(),
            "a blocked contact is refused, window or not"
        );
    }

    /// 191 (D2, D7): before `contacts_ready` a staged contact with a live binding is refused,
    /// and so is a ceremony (the deny set is whole only after the first pass); after it the
    /// contact is admitted and a stranger in a window may pair; a denied endpoint is refused
    /// even for a ceremony, a removed one only as a contact.
    #[test]
    fn the_gate_refuses_everyone_before_contacts_ready_and_denied_endpoints_first() {
        let mut staged = Staged::default();
        staged.contacts.insert(
            "bob".into(),
            crate::config::ContactFacts {
                id: "r1".into(),
                eid: "bob-eid".into(),
                eids: vec!["bob-eid".into()],
                display: "bob".into(),
                machine: String::new(),
                first_contact: BTreeMap::new(),
                blocked: false,
                paired_ms: 1,
                addrs: Vec::new(),
            },
        );
        let mut live = Live::display(staged);
        live.bindings.insert("bob".into(), Uuid::from_u128(7));
        assert_eq!(iroh_gate(&live, "bob-eid", true, false), IrohGate::Refuse);
        assert_eq!(iroh_gate(&live, "stranger", true, false), IrohGate::Refuse);
        assert!(matches!(
            iroh_gate(&live, "bob-eid", false, true),
            IrohGate::Contact { alias, .. } if alias == "bob"
        ));
        assert_eq!(iroh_gate(&live, "stranger", true, true), IrohGate::Pair);
        live.staged.deny.insert("stranger".into());
        assert_eq!(iroh_gate(&live, "stranger", true, true), IrohGate::Refuse);
        live.staged.removed.insert("gone".into());
        assert_eq!(
            iroh_gate(&live, "gone", true, true),
            IrohGate::Pair,
            "it may pair again"
        );
        live.staged.removed.insert("bob-eid".into());
        assert_eq!(iroh_gate(&live, "bob-eid", true, true), IrohGate::Refuse);
    }
}
