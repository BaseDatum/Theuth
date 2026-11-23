//! `theuth/share/1` (210 §5.3): the protocol handler that admits engine connections, and the control API, one
//! HTTP/1.1 request per bi-stream the engine opens, served by axum through hyper as a2a's `serve_http` does.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Extension, Path, RawQuery};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use futures::Stream;
use hyper_util::rt::{TokioIo, TokioTimer};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use serde::Serialize;
use serde::de::DeserializeOwned;
use share_proto::names::{self, HostError};
use share_proto::version::{client_ok, edge_agent, protocol_ok};
use share_proto::{
    Admission, CsrQuery, Delegation, EdgeIdentity, ErrorBody, ErrorCode, Hello, Info,
    InterstitialRule, KillReason, MeshEngine, MeshStatus, NameAvailability, NameGranted, NameInfo,
    NameKind, NameReleased, NameRequest, NameState, PROTOCOL_MAJOR, PublicKey, Rekey,
    RekeyAccepted, RekeyEntry, RevokeRequest, RevokeResult, RouteEntry, RoutesPut,
    SUPPORTED_MAJORS, SignedRekey, SpkiHash, StopAll, TransferAccepted, TransferRequest, Welcome,
    WelcomeLimits, paths, LimitKind, LimitScope,
};
use tower::ServiceExt as _;

use crate::state::{Edge, Route, Session, Tunnel, now_ms};
use crate::store::{Claimed, NameRow, NewName, Rekeyed};

/// Header read deadline of one control request.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole-exchange deadline of a control request other than `GET /v1/events`.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The `theuth/share/1` handler.
#[derive(Debug, Clone)]
pub struct ShareProtocol {
    /// Shared edge state.
    pub edge: Arc<Edge>,
}

impl ProtocolHandler for ShareProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let edge = self.edge.clone();
        // D4 admission at connect: pause, a banned engine key, the pre-`hello` cap.
        if edge.paused.load(Ordering::Relaxed) {
            conn.close(2u32.into(), b"paused");
            return Ok(());
        }
        let peer = conn.remote_id().to_string();
        if matches!(edge.store.ban_reason("key", &peer).await, Ok(Some(_))) {
            conn.close(3u32.into(), b"banned");
            return Ok(());
        }
        let Ok(permit) = edge.pre_hello.clone().try_acquire_owned() else {
            conn.close(4u32.into(), b"busy");
            return Ok(());
        };
        let tunnel = Tunnel::new(conn, Some(permit), &edge.live().limits);
        serve(edge, tunnel).await;
        Ok(())
    }
}

/// The life of one engine connection: control streams until close, the `hello` deadline and delegation expiry.
async fn serve(edge: Arc<Edge>, tunnel: Arc<Tunnel>) {
    let api = api();
    let hello_deadline =
        tokio::time::Instant::now() + Duration::from_secs(edge.live().limits.hello_timeout_secs);
    loop {
        let wake = match tunnel.session.lock().expect("session lock").as_ref() {
            None => hello_deadline,
            Some(s) => {
                let left = s.expires_ms.saturating_sub(now_ms());
                tokio::time::Instant::now() + Duration::from_millis(left)
            }
        };
        tokio::select! {
            s = tunnel.conn.accept_bi() => match s {
                Ok((send, recv)) => {
                    tokio::spawn(serve_stream(api.clone(), edge.clone(), tunnel.clone(), send, recv));
                }
                Err(_) => break,
            },
            _ = tokio::time::sleep_until(wake) => {
                let has = tunnel.session.lock().expect("session lock").is_some();
                if !has {
                    tunnel.conn.close(5u32.into(), b"hello_required");
                    break;
                }
                if tunnel.session(now_ms()).is_none() {
                    // §5.7: at expiry without a renewed delegation, one `kill`, then stop serving (FIX-EDGE M10:
                    // staying in the loop re-armed an already-elapsed timer and killed on every pass).
                    tunnel.kill(None, KillReason::DelegationExpired);
                    break;
                }
            }
        }
    }
    edge.detach(&tunnel);
}

/// One HTTP/1.1 request on one bi-stream.
async fn serve_stream(
    api: Router,
    edge: Arc<Edge>,
    tunnel: Arc<Tunnel>,
    send: SendStream,
    recv: RecvStream,
) {
    let sse = Arc::new(AtomicBool::new(false));
    let flag = sse.clone();
    let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let (api, edge, tunnel, flag) = (api.clone(), edge.clone(), tunnel.clone(), flag.clone());
        async move {
            let mut req = req.map(Body::new);
            // FIX4-EDGE #7: control requests per tunnel; every endpoint counts, `GET /v1/events` included.
            if tunnel.control_rate.check().is_err() {
                return Ok::<_, Infallible>(err(ErrorCode::RateLimited).into_response());
            }
            if req.uri().path() == paths::EVENTS {
                flag.store(true, Ordering::Relaxed);
            }
            req.extensions_mut().insert(Ctx { edge: edge.clone(), tunnel });
            let resp = api.oneshot(req).await.unwrap_or_else(|e| match e {});
            if let Some(Refused(b)) = resp.extensions().get::<Refused>() {
                edge.ops.refused(b, now_ms());
            }
            Ok::<_, Infallible>(resp)
        }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .keep_alive(false)
        .serve_connection(TokioIo::new(tokio::io::join(recv, send)), svc);
    tokio::pin!(conn);
    tokio::select! {
        r = &mut conn => { if let Err(e) = r { tracing::debug!("control request: {e}"); } }
        _ = tokio::time::sleep(REQUEST_TIMEOUT) => {
            // The event stream lives as long as the connection; anything else is cut.
            if sse.load(Ordering::Relaxed) {
                let _ = conn.await;
            }
        }
    }
}

/// Per-request context.
#[derive(Clone)]
struct Ctx {
    edge: Arc<Edge>,
    tunnel: Arc<Tunnel>,
}

impl Ctx {
    fn session(&self) -> Result<Session, ApiError> {
        self.tunnel
            .session(now_ms())
            .ok_or_else(|| ErrorBody::new(ErrorCode::HelloRequired).into())
    }

    fn domain(&self) -> &str {
        &self.edge.cfg.domain
    }
}

/// An error answer: the body's status, or an override.
struct ApiError(Box<ErrorBody>, Option<StatusCode>);

impl From<ErrorBody> for ApiError {
    fn from(b: ErrorBody) -> Self {
        ApiError(Box::new(b), None)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::warn!("control API: {e:#}");
        let mut b = ErrorBody::new(ErrorCode::Internal);
        b.message = Some("internal error".into());
        ApiError(Box::new(b), None)
    }
}

/// The error body of an answer, for the refusal counters (§9.5 monitoring, the brake).
#[derive(Clone)]
struct Refused(ErrorBody);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.1.unwrap_or_else(|| {
            StatusCode::from_u16(self.0.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        });
        let refused = crate::ops::is_limit_refusal(&self.0).then(|| Refused((*self.0).clone()));
        let mut resp = (status, axum::Json(self.0)).into_response();
        if let Some(r) = refused {
            resp.extensions_mut().insert(r);
        }
        resp
    }
}

/// §7.6: name and CSR requests need the live terms revision accepted at `hello`; else `403 terms_required`
/// with the terms URL as the message.
fn terms(e: &Edge, s: &Session) -> Result<(), ApiError> {
    let live = e.live();
    if live.terms_rev.is_empty() || s.terms_rev.as_deref() == Some(live.terms_rev.as_str()) {
        return Ok(());
    }
    let mut b = ErrorBody::new(ErrorCode::TermsRequired);
    b.message = Some(live.terms_url.clone());
    Err(b.into())
}

fn err(code: ErrorCode) -> ApiError {
    ErrorBody::new(code).into()
}

fn not_yet(what: &str) -> ApiError {
    let mut b = ErrorBody::new(ErrorCode::Internal);
    b.message = Some(format!("{what} is not available on this edge yet"));
    ApiError(Box::new(b), Some(StatusCode::NOT_IMPLEMENTED))
}

type ApiResult = Result<Response, ApiError>;

fn json<T: Serialize>(status: StatusCode, body: &T) -> ApiResult {
    Ok((status, axum::Json(body)).into_response())
}

fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| ErrorBody::invalid("json").into())
}

fn api() -> Router {
    Router::new()
        .route(paths::INFO, get(info))
        .route(paths::HELLO, post(hello))
        .route(paths::NAMES, post(allocate_random))
        .route(
            paths::NAME,
            get(availability).put(claim_reserved).delete(release),
        )
        .route(paths::NAME_TRANSFER, post(transfer))
        .route(paths::NAME_CSR, post(csr))
        .route(paths::NAME_CERT, get(cert))
        .route(paths::ROUTES, put(put_routes))
        .route(paths::EVENTS, get(events))
        .route(paths::MESH, get(mesh_status))
        .route(paths::MESH_STOP, post(mesh_stop))
        .route(paths::MESH_REVOKE, post(mesh_revoke))
        .route(paths::MESH_REKEY, post(mesh_rekey))
        .fallback(|| async { err(ErrorCode::NotFound) })
        .layer(DefaultBodyLimit::max(share_proto::limits::MAX_JSON_BODY))
}

/// `GET /v1/info`, the one request answered before `hello`; `info_per_ip_min` per client network.
async fn info(Extension(c): Extension<Ctx>) -> ApiResult {
    let e = &c.edge;
    if e.info_rate.check_key(&c.tunnel.net).is_err() {
        return Err(err(ErrorCode::RateLimited));
    }
    json(
        StatusCode::OK,
        &Info {
            v: PROTOCOL_MAJOR,
            edge: edge_agent(crate::state::VERSION),
            domain: e.cfg.domain.clone(),
            edge_id: e.id,
            now_ms: now_ms(),
            min_client: e.cfg.min_client.clone(),
            admission: e.admission(),
            terms_url: e.live().terms_url.clone(),
            terms_rev: e.live().terms_rev.clone(),
            abuse_url: e.live().abuse_url.clone(),
            full: e.full(),
        },
    )
}

/// `POST /v1/hello` (§5.7): protocol and client version, delegation check, bans, rekey, revoke, admission,
/// engine count; then `welcome`.
async fn hello(Extension(c): Extension<Ctx>, body: Bytes) -> ApiResult {
    let e = c.edge.clone();
    let h: Hello = parse(&body)?;
    // A protocol major this edge does not serve is `426 protocol_mismatch {supported}`; a client below
    // `min_client` is `426 client_too_old`. Both before anything else is read.
    if !protocol_ok(h.v) {
        let mut b = ErrorBody::protocol_mismatch(SUPPORTED_MAJORS.to_vec());
        b.message = Some(format!("protocol v{} is not served here", h.v));
        return Err(b.into());
    }
    if !client_ok(&h.client, &e.cfg.min_client) {
        let mut b = ErrorBody::new(ErrorCode::ClientTooOld);
        b.message = Some(format!("client {:?} is older than min_client {}", h.client, e.cfg.min_client));
        return Err(b.into());
    }
    let now = now_ms();
    let ident = EdgeIdentity {
        edge: e.id,
        domain: e.cfg.domain.clone(),
    };
    let max_ttl = u64::try_from(e.cfg.delegation_max_ttl.as_millis()).unwrap_or(u64::MAX);
    let d = Delegation::verify(&h.delegation, &c.tunnel.engine, &ident, now, max_ttl).map_err(
        |de| {
            let mut b = ErrorBody::new(de.code());
            b.now_ms = de.now_ms();
            ApiError::from(b)
        },
    )?;
    if d.mesh == d.engine {
        return Err(err(ErrorCode::BadDelegation));
    }
    if let Some(prev) = c.tunnel.mesh()
        && prev != d.mesh
    {
        // A connection is bound to one mesh; a different mesh needs a new connection.
        return Err(err(ErrorCode::BadDelegation));
    }
    if e.paused.load(Ordering::Relaxed) || e.draining.load(Ordering::Relaxed) {
        return Err(err(ErrorCode::AdmissionClosed));
    }
    let mesh = d.mesh.to_string();
    let engine = d.engine.to_string();
    for (kind, value) in [
        ("mesh", mesh.as_str()),
        ("key", engine.as_str()),
        ("net", c.tunnel.net.as_str()),
    ] {
        if e.store.ban_reason(kind, value).await?.is_some() {
            return Err(err(ErrorCode::Banned));
        }
    }
    let known = e.store.mesh(&mesh).await?;
    if known.as_ref().is_some_and(|m| m.rekeyed_to.is_some()) {
        // The end of the chain: the mesh id the edge holds now (§5.7).
        let to = e.store.current_mesh(&mesh).await?;
        let to: PublicKey = to.parse().map_err(|_| anyhow::anyhow!("bad rekeyed_to"))?;
        return Err(ErrorBody::rekeyed(to).into());
    }
    if e.store.revoked(&mesh, &engine).await?.is_some() {
        return Err(err(ErrorCode::Revoked));
    }
    let mut invited = false;
    // Checked here, consumed in the attach transaction below (FIX4-EDGE #20): a refusal in between spends no use.
    let mut invite_hash = None;
    if known.is_none() {
        match e.admission() {
            Admission::Open => {}
            Admission::Closed => return Err(err(ErrorCode::AdmissionClosed)),
            Admission::Invite => {
                let hash = h.invite.as_ref().map(|inv| inv.hash());
                let ok = match &hash {
                    Some(hash) => e.store.invite_live(hash, now).await?,
                    None => false,
                };
                if !ok {
                    return Err(err(ErrorCode::InviteRequired));
                }
                invite_hash = hash;
                invited = true;
            }
        }
        // TODO(210-P2): `allowlist` admission.
        // §9.4: meshes first seen per network tier per day, or the paced relayed-newcomer bucket; invited
        // meshes leave every per-network onboarding count.
        if !invited {
            crate::limits::new_mesh(&e, &c.tunnel.net, now)
                .await?
                .map_err(ApiError::from)?;
        }
        e.ops.new_mesh(now);
    } else if !known.as_ref().is_some_and(|m| m.invited != 0)
        && !crate::limits::is_relayed(&c.tunnel.net)
        && e.store.rehome_relayed(&mesh, &c.tunnel.net).await?
    {
        // §9.4: a relayed newcomer that now shows a direct path is counted against that network. It is already
        // admitted, so an exhausted network count is logged, not answered.
        if let Err(b) = crate::limits::new_mesh(&e, &c.tunnel.net, now).await? {
            tracing::info!(%mesh, net = %c.tunnel.net, code = %b.code, "relayed mesh re-counted over its network's limit");
        }
    }
    // §9.4: engine keys first seen per mesh per UTC day.
    if !e.store.engine_known(&mesh, &engine).await?
        && e.store
            .engines_first_seen_since(&mesh, crate::limits::day_start(now))
            .await?
            >= u64::from(e.live().limits.engine_keys_per_mesh_day)
    {
        return Err(ErrorBody::limited(ErrorCode::EnginesQuota, LimitScope::Mesh, LimitKind::Engines).into());
    }
    let others = e
        .mesh_tunnels(&d.mesh)
        .iter()
        .filter(|t| t.engine != d.engine)
        .count();
    if others >= e.cfg.engines_per_mesh as usize {
        return Err(ErrorBody::limited(ErrorCode::EnginesQuota, LimitScope::Mesh, LimitKind::Engines).into());
    }
    if let Some(b) = crate::limits::tunnels_refusal(&e, &c.tunnel.net, &d.engine) {
        return Err(b.into());
    }
    let mesh_invited = invited || known.as_ref().is_some_and(|m| m.invited != 0);
    match &invite_hash {
        Some(hash) => {
            if !e.store.attach_invited(&mesh, &engine, &c.tunnel.net, hash, now).await? {
                // Spent or expired since the check above (a concurrent `hello` took the last use).
                return Err(err(ErrorCode::InviteRequired));
            }
        }
        None => e.store.attach(&mesh, &engine, &c.tunnel.net, invited, now).await?,
    }
    // An edge without terms needs no acceptance.
    let terms_ok =
        e.live().terms_rev.is_empty() || h.terms_rev.as_deref() == Some(e.live().terms_rev.as_str());
    *c.tunnel.session.lock().expect("session lock") = Some(Session {
        mesh: d.mesh,
        expires_ms: d.expires_ms,
        terms_ok,
        terms_rev: h.terms_rev.clone(),
        hello_ms: now,
    });
    c.tunnel.pre_hello.lock().expect("permit lock").take();
    // §9.6: shaping at `tunnel_mbit`, or `throttle_mbit` for a non-invited mesh while egress is over pace.
    c.tunnel.invited.store(mesh_invited, Ordering::Relaxed);
    c.tunnel.shaper.set_rate(e.meter.rate_for(&e, mesh_invited));
    let throttled = !mesh_invited && e.meter.throttled.load(Ordering::Relaxed);
    // §5.3: one tunnel per engine key; a new one replaces the old.
    if let Some(old) = e.tunnels.insert(d.engine, c.tunnel.clone())
        && !Arc::ptr_eq(&old, &c.tunnel)
    {
        e.detach(&old);
        old.conn.close(6u32.into(), b"replaced");
    }
    let queue = match e.certs.get() {
        Some(certs) => crate::queue::mesh_positions(&e, certs.ca.id(), &mesh, now).await?,
        None => HashMap::new(),
    };
    let names = e
        .store
        .held_names(c.domain(), &mesh)
        .await?
        .iter()
        .map(|r| {
            let mut n = name_info(r);
            n.queue = queue.get(&r.name).copied();
            n
        })
        .collect();
    let stop_all_ms = known
        .and_then(|m| m.stop_all_ms)
        .and_then(|v| u64::try_from(v).ok());
    let revoked = e
        .store
        .engines(&mesh)
        .await?
        .into_iter()
        .filter(|r| r.revoked_ms.is_some())
        .filter_map(|r| r.engine.parse().ok())
        .collect();
    let admission = e.admission();
    json(
        StatusCode::OK,
        &Welcome {
            v: PROTOCOL_MAJOR,
            now_ms: now,
            min_client: e.cfg.min_client.clone(),
            mesh: d.mesh,
            names,
            routes: e.mesh_routes(&d.mesh),
            revoked,
            stop_all_ms,
            domain: e.cfg.domain.clone(),
            limits: WelcomeLimits {
                interstitial: if admission == Admission::Open {
                    InterstitialRule::Required
                } else {
                    InterstitialRule::Optional
                },
                reclaim_cooldown_ms: u64::try_from(e.cfg.reclaim_cooldown.as_millis())
                    .unwrap_or(u64::MAX),
                reserved_per_mesh: e.cfg.reserved_per_mesh,
                engines_per_mesh: e.cfg.engines_per_mesh,
                routes_per_tunnel: e.live().limits.routes_per_tunnel,
                conns_per_tunnel: e.live().limits.conns_per_tunnel,
                delegation_max_ttl_ms: max_ttl,
                tunnel_mbit: e.live().limits.tunnel_mbit,
                throttle_mbit: e.live().limits.throttle_mbit,
                ..WelcomeLimits::default()
            },
            terms_url: e.live().terms_url.clone(),
            abuse_url: e.live().abuse_url.clone(),
            terms_rev: e.live().terms_rev.clone(),
            terms_ok,
            full: e.full(),
            // §5.5: an edge promoted to its standby key retires the old live key.
            retire: e.retire.get().cloned(),
            throttled,
        },
    )
}

pub(crate) fn name_info(r: &NameRow) -> NameInfo {
    NameInfo {
        name: r.name.clone(),
        kind: if r.kind == "reserved" {
            NameKind::Reserved
        } else {
            NameKind::Random
        },
        state: match r.state.as_str() {
            "queued" => NameState::Queued,
            "slotted" => NameState::Slotted,
            "valid" => NameState::Valid,
            "released" => NameState::Released,
            "banned" => NameState::Banned,
            _ => NameState::Allocated,
        },
        ca: r.ca.clone(),
        spki: r
            .csr_spki
            .as_deref()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .map(SpkiHash),
        not_after: r.not_after_ms.and_then(|v| u64::try_from(v).ok()),
        // Filled by `hello` from the slot queue (§6.5).
        queue: None,
        transfer_to: r.transfer_to.as_deref().and_then(|t| t.parse().ok()),
        transfer_until_ms: r.transfer_until_ms.and_then(|v| u64::try_from(v).ok()),
    }
}

/// `GET /v1/names/{name}`; rate limited per tunnel and client network tier ([`crate::limits::lookup`]).
async fn availability(Extension(c): Extension<Ctx>, Path(name): Path<String>) -> ApiResult {
    let s = c.session()?;
    if let Some(b) = crate::limits::lookup(&c.edge, &c.tunnel.net, &c.tunnel.engine) {
        return Err(b.into());
    }
    let reason = |r: &str| NameAvailability {
        available: false,
        reason: Some(r.into()),
        reclaim_after_ms: None,
    };
    if names::check_random(&name).is_err()
        && let Err(e) = names::check_reserved(&name, &c.edge.live().reserved_words)
    {
        return json(StatusCode::OK, &reason(e.reason()));
    }
    if c.edge.store.ban_reason("name", &name).await?.is_some() {
        return json(StatusCode::OK, &reason("banned"));
    }
    let mesh = s.mesh.to_string();
    let answer = match c.edge.store.name(c.domain(), &name).await? {
        None => NameAvailability {
            available: true,
            reason: None,
            reclaim_after_ms: None,
        },
        Some(r) if r.state == "banned" => reason("banned"),
        Some(r) if r.owner.as_deref() == Some(mesh.as_str()) => NameAvailability {
            available: true,
            reason: None,
            reclaim_after_ms: None,
        },
        // The receiving mesh of a pending transfer may claim it (§5.2 D20).
        Some(r)
            if r.held()
                && r.transfer_to.as_deref() == Some(mesh.as_str())
                && r.transfer_until_ms.is_some_and(|u| u > now_i64()) =>
        {
            NameAvailability {
                available: true,
                reason: None,
                reclaim_after_ms: None,
            }
        }
        Some(r) if r.held() => reason("taken"),
        Some(r) if r.reclaim_after_ms.is_some_and(|t| t > now_i64()) => {
            let mut a = reason("cooldown");
            a.reclaim_after_ms = r.reclaim_after_ms.and_then(|v| u64::try_from(v).ok());
            a
        }
        Some(_) => NameAvailability {
            available: true,
            reason: None,
            reclaim_after_ms: None,
        },
    };
    json(StatusCode::OK, &answer)
}

fn now_i64() -> i64 {
    i64::try_from(now_ms()).unwrap_or(i64::MAX)
}

/// The claim rules of §5.2 shared by random allocation and reserved claims; an existing hold by this mesh is
/// idempotent. Ban, owner, cooldown and quota are decided by [`crate::store::Store::claim`] in one transaction
/// (FIX-EDGE M5).
async fn claim(c: &Ctx, s: &Session, name: &str, kind: NameKind) -> Result<(), ApiError> {
    terms(&c.edge, s)?;
    let e = &c.edge;
    if e.store.ban_reason("name", name).await?.is_some() {
        return Err(err(ErrorCode::Banned));
    }
    let mesh = s.mesh.to_string();
    // §9.4, §6.5: `edge_full`, the per-network claim rate and reserved holds, the paced open-names bucket.
    // A re-claim of a name this mesh holds is idempotent and costs nothing.
    let now = now_ms();
    let mine = e
        .store
        .name(c.domain(), name)
        .await?
        .is_some_and(|r| r.held() && r.owner.as_deref() == Some(mesh.as_str()));
    let taken = if mine {
        crate::limits::Taken::default()
    } else {
        let min_age = u64::try_from(e.live().limits.reserved_min_mesh_age.as_millis()).unwrap_or(u64::MAX);
        if kind == NameKind::Reserved && min_age > 0 {
            let created = e
                .store
                .mesh_detail(&mesh)
                .await?
                .map_or(now, |m| u64::try_from(m.created_ms).unwrap_or(now));
            let ready = created.saturating_add(min_age);
            if ready > now {
                let mut b = ErrorBody::limited(ErrorCode::Quota, LimitScope::Mesh, LimitKind::Reserved);
                b.retry_after_s = Some((ready - now).div_ceil(1000));
                return Err(b.into());
            }
        }
        let invited = e.store.mesh(&mesh).await?.is_some_and(|m| m.invited != 0);
        crate::limits::new_name(e, invited, &c.tunnel.net, kind, now)
            .await?
            .map_err(ApiError::from)?
    };
    let outcome = e
        .store
        .claim(
            NewName {
                domain: c.domain(),
                name,
                kind: if kind == NameKind::Reserved {
                    "reserved"
                } else {
                    "random"
                },
                owner: &mesh,
                now_ms: now_i64(),
                net: &c.tunnel.net,
            },
            e.cfg.reserved_per_mesh,
        )
        .await?;
    if !matches!(outcome, Claimed::Granted) {
        taken.refund(e, now).await?;
    }
    match outcome {
        Claimed::Granted | Claimed::AlreadyHeld => {
            // Routing objects before the certificate job needs them; never blocks the claim.
            crate::k8s::granted(&c.edge, name);
            Ok(())
        }
        Claimed::Transferred => {
            // §5.2 D20: the giving mesh's routes under the name go now (announced to it); the old chain is
            // queued for revocation in the claim's transaction, the revoker woken. The k8s per-name objects
            // stay: the name keeps serving, under its new mesh.
            let to = s.mesh;
            e.drop_routes(|_, r| r.name == name && r.mesh != to);
            crate::k8s::granted(&c.edge, name);
            if let Some(certs) = e.certs.get() {
                certs.wake.notify_one();
            }
            Ok(())
        }
        Claimed::Banned => Err(err(ErrorCode::Banned)),
        Claimed::Taken => Err(err(ErrorCode::NameTaken)),
        Claimed::Quota => Err(err(ErrorCode::Quota)),
        Claimed::Cooldown(t) => {
            let mut b = ErrorBody::new(ErrorCode::Cooldown);
            b.reclaim_after_ms = u64::try_from(t).ok();
            Err(b.into())
        }
    }
}

/// `POST /v1/names {name}`: a client-drawn random name.
async fn allocate_random(Extension(c): Extension<Ctx>, body: Bytes) -> ApiResult {
    let s = c.session()?;
    let req: NameRequest = parse(&body)?;
    names::check_random(&req.name).map_err(|e| ApiError::from(ErrorBody::invalid(e.reason())))?;
    claim(&c, &s, &req.name, NameKind::Random).await?;
    json(StatusCode::CREATED, &NameGranted { name: req.name })
}

/// `PUT /v1/names/{name}`: claim a reserved name. TODO(210-P2): `{handover}` body.
async fn claim_reserved(Extension(c): Extension<Ctx>, Path(name): Path<String>) -> ApiResult {
    let s = c.session()?;
    names::check_reserved(&name, &c.edge.live().reserved_words)
        .and_then(|()| names::check_brand(&name, &c.edge.live().brands))
        .map_err(|e| ApiError::from(ErrorBody::invalid(e.reason())))?;
    // TODO(210-M2): an online transfer (`transfer_to` = this mesh) skips the cooldown and revokes the old cert.
    claim(&c, &s, &name, NameKind::Reserved).await?;
    json(StatusCode::OK, &NameGranted { name })
}

/// `DELETE /v1/names/{name}`: release with the cooldown `max(now + reclaim_cooldown, not_after)`.
async fn release(Extension(c): Extension<Ctx>, Path(name): Path<String>) -> ApiResult {
    let s = c.session()?;
    let e = &c.edge;
    let mesh = s.mesh.to_string();
    if e.store.name(c.domain(), &name).await?.is_none() {
        return Err(err(ErrorCode::NotFound));
    }
    let cooldown = u64::try_from(e.cfg.reclaim_cooldown.as_millis()).unwrap_or(u64::MAX);
    let Some(reclaim_after_ms) = e
        .store
        .release(c.domain(), &name, &mesh, now_ms(), cooldown)
        .await?
    else {
        return Err(err(ErrorCode::NotOwner));
    };
    e.drop_routes(|_, route| route.name == name && route.mesh == s.mesh);
    crate::k8s::gone(&c.edge, &name);
    // §6.3: revoke (`cessationOfOperation`), delete the D17 records, drop a queued job; in the background,
    // since DNS calls take seconds and reconciliation at start catches what a crash leaves.
    let edge = c.edge.clone();
    let n = name.clone();
    tokio::spawn(async move {
        crate::certs::retire(&edge, &n, crate::acme::Reason::CessationOfOperation).await;
    });
    json(
        StatusCode::OK,
        &NameReleased {
            name,
            reclaim_after_ms,
        },
    )
}

/// `POST /v1/names/{name}/transfer {to_mesh}` (§5.2 D20): offer a held name to another mesh for
/// `TRANSFER_TTL_MS`; the receiver claims it with the plain `PUT`, with no cooldown. A newer offer replaces the
/// previous one; a release cancels it. `to_mesh` is resolved through its rekey chain.
async fn transfer(
    Extension(c): Extension<Ctx>,
    Path(name): Path<String>,
    body: Bytes,
) -> ApiResult {
    let s = c.session()?;
    let e = &c.edge;
    let req: TransferRequest = parse(&body)?;
    let to = e.store.current_mesh(&req.to_mesh.to_string()).await?;
    if to == s.mesh.to_string() {
        return Err(ErrorBody::invalid("to_mesh").into());
    }
    let until = now_ms().saturating_add(share_proto::limits::TRANSFER_TTL_MS);
    if !e
        .store
        .offer_transfer(c.domain(), &name, &s.mesh.to_string(), &to, until)
        .await?
    {
        return Err(err(ErrorCode::NotOwner));
    }
    json(
        StatusCode::OK,
        &TransferAccepted {
            transfer_until_ms: until,
        },
    )
}

/// `POST /v1/names/{name}/csr[?rotate=true[&compromised=true]]`, body = CSR DER (§6.2, §6.3).
async fn csr(
    Extension(c): Extension<Ctx>,
    Path(name): Path<String>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> ApiResult {
    let s = c.session()?;
    terms(&c.edge, &s)?;
    if c.edge.certs.get().is_none() {
        return Err(not_yet("certificate issuance"));
    }
    let q = csr_query(query.as_deref().unwrap_or(""));
    match crate::certs::submit(&c.edge, &s.mesh, &name, &body, q).await? {
        Ok(crate::certs::Submitted::Chain(chain)) => json(StatusCode::OK, &chain),
        Ok(crate::certs::Submitted::Pending(p)) => json(StatusCode::ACCEPTED, &p),
        Err(b) => Err(b.into()),
    }
}

/// The CSR post's query: `rotate=true`, and `compromised=true` only together with it (CONTRACT2).
fn csr_query(q: &str) -> CsrQuery {
    let has = |k: &str| q.split('&').any(|kv| kv == format!("{k}=true"));
    let rotate = has("rotate");
    CsrQuery {
        rotate,
        compromised: rotate && has("compromised"),
    }
}

/// `GET /v1/names/{name}/cert`: the current chain, `404 not_found` before the first.
async fn cert(Extension(c): Extension<Ctx>, Path(name): Path<String>) -> ApiResult {
    let s = c.session()?;
    match crate::certs::chain(&c.edge, &s.mesh, &name).await? {
        Ok(chain) => json(StatusCode::OK, &chain),
        Err(b) => Err(b.into()),
    }
}

/// `PUT /v1/routes {hosts}`: this engine's full set; all or nothing (§5.4).
async fn put_routes(Extension(c): Extension<Ctx>, body: Bytes) -> ApiResult {
    let s = c.session()?;
    let e = &c.edge;
    let req: RoutesPut = parse(&body)?;
    after_stop_all(e, &s).await?;
    // §7.4 per share: a host whose share went live at or before the mesh's last Stop all is refused, nothing
    // applied, naming those hosts.
    if let Some(stop) = stop_all_of(e, &s.mesh).await? {
        let stopped = req.stopped_hosts(stop);
        if !stopped.is_empty() {
            return Err(ErrorBody::stopped_all(stop, stopped).into());
        }
    }
    if req.hosts.len() > e.live().limits.routes_per_tunnel as usize {
        return Err(ErrorBody::limited(ErrorCode::Quota, LimitScope::Tunnel, LimitKind::Routes).into());
    }
    // §9.6: a mesh or network over its day's bytes publishes nothing new until the UTC day rolls.
    let mesh_s = s.mesh.to_string();
    if !req.hosts.is_empty() && e.meter.over_budget(e, &mesh_s, &c.tunnel.net) {
        let scope = if e.meter.over.contains_key(&format!("mesh:{mesh_s}")) {
            LimitScope::Mesh
        } else {
            LimitScope::Net
        };
        return Err(ErrorBody::limited(ErrorCode::Quota, scope, LimitKind::Bytes).into());
    }
    let held: Vec<String> = e
        .store
        .held_names(c.domain(), &s.mesh.to_string())
        .await?
        .into_iter()
        .map(|r| r.name)
        .collect();
    let mut wanted: Vec<(String, String)> = Vec::with_capacity(req.hosts.len());
    for h in &req.hosts {
        let parts = names::parse_host(h, c.domain()).map_err(|he| {
            ApiError::from(ErrorBody::invalid(match he {
                HostError::Label(n) => n.reason(),
                _ => "host",
            }))
        })?;
        if !held.contains(&parts.name) {
            return Err(err(ErrorCode::NotOwner));
        }
        let host = names::host(&parts.name, parts.label.as_deref(), c.domain());
        if !wanted.iter().any(|(w, _)| *w == host) {
            wanted.push((host, parts.name));
        }
    }
    evict_stale_routes(e, c.domain(), &s.mesh, &wanted).await?;
    let conflicts: Vec<RouteEntry> = wanted
        .iter()
        .filter_map(|(h, _)| {
            e.routes
                .get(h)
                .filter(|r| r.tunnel.engine != c.tunnel.engine)
                .map(|r| RouteEntry {
                    host: h.clone(),
                    engine: r.tunnel.engine,
                })
        })
        .collect();
    if !conflicts.is_empty() {
        return Err(ErrorBody::route_conflict(conflicts).into());
    }
    let tunnel = c.tunnel.clone();
    e.drop_routes(|h, r| Arc::ptr_eq(&r.tunnel, &tunnel) && !wanted.iter().any(|(w, _)| w == h));
    for (host, name) in wanted {
        // The host's `stats` counters carry over a re-put by the same tunnel.
        let stats = e
            .routes
            .get(&host)
            .filter(|r| Arc::ptr_eq(&r.tunnel, &tunnel))
            .map(|r| r.stats.clone())
            .unwrap_or_default();
        let prev = e.routes.insert(
            host.clone(),
            Route {
                name,
                mesh: s.mesh,
                tunnel: tunnel.clone(),
                stats,
            },
        );
        if prev.is_none_or(|p| !Arc::ptr_eq(&p.tunnel, &tunnel)) {
            e.announce(&s.mesh, &host, Some(tunnel.engine));
        }
    }
    // A Stop all that landed while this request ran: `mesh_stop` writes the store before dropping routes, so
    // either its drop removed what was inserted above, or this read sees it.
    if let Err(stale) = after_stop_all(e, &s).await {
        e.drop_routes(|_, r| Arc::ptr_eq(&r.tunnel, &tunnel));
        return Err(stale);
    }
    // FIX4-EDGE #4, same pattern: a release, ban or transfer that landed while this request ran writes the store
    // before dropping routes, so either its drop removed what was inserted above, or this read sees it.
    let still: Vec<String> = e
        .store
        .held_names(c.domain(), &mesh_s)
        .await?
        .into_iter()
        .map(|r| r.name)
        .collect();
    let lost = e.drop_routes(|_, r| Arc::ptr_eq(&r.tunnel, &tunnel) && !still.contains(&r.name));
    if !lost.is_empty() {
        return Err(err(ErrorCode::NotOwner));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// FIX-EDGE M7 (edge half of §7.4): routes come only from a session that began after the mesh's last Stop
/// all. An engine attached before it must reconnect, take `welcome.stop_all_ms`, stop every share with
/// `live_ms <= stop_all_ms`, and publish again. The per-share rule itself is the client's: `RoutesPut` does
/// not carry when each share was published.
async fn after_stop_all(e: &Edge, s: &Session) -> Result<(), ApiError> {
    let stop = e
        .store
        .mesh(&s.mesh.to_string())
        .await?
        .and_then(|m| m.stop_all_ms)
        .and_then(|v| u64::try_from(v).ok());
    match stop {
        Some(stop) if s.hello_ms <= stop => {
            let mut b = ErrorBody::new(ErrorCode::HelloRequired);
            b.message = Some(format!("Stop all at {stop}: reconnect, then publish again"));
            Err(b.into())
        }
        _ => Ok(()),
    }
}

/// `GET /v1/events`: SSE held for the connection; `stats` every 30 s come from the metering loop (§9.6).
async fn events(
    Extension(c): Extension<Ctx>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    c.session()?;
    let rx = c.tunnel.events.subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    return Some((Ok(SseEvent::default().event(ev.name()).data(ev.data())), rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// The mesh's last Stop all.
async fn stop_all_of(e: &Edge, mesh: &PublicKey) -> anyhow::Result<Option<u64>> {
    Ok(e.store
        .mesh(&mesh.to_string())
        .await?
        .and_then(|m| m.stop_all_ms)
        .and_then(|v| u64::try_from(v).ok()))
}

/// FIX4-EDGE #4: a host routed by another mesh's tunnel is stale when `mesh` holds the name now (e.g. after a
/// transfer whose route drop raced a re-put): evict it instead of refusing the owner. FIX5-EDGE R3: the owner
/// is read again here, just before the eviction, so a release or transfer since the caller's `held` read evicts
/// nobody; such a host keeps the other mesh's route (the caller answers `route_conflict`). `wanted` is
/// `(host, name)`. Returns the hosts evicted.
pub async fn evict_stale_routes(
    e: &Edge,
    domain: &str,
    mesh: &PublicKey,
    wanted: &[(String, String)],
) -> anyhow::Result<Vec<String>> {
    let mesh_s = mesh.to_string();
    let mut evictable: Vec<&str> = Vec::new();
    for (host, name) in wanted {
        if !e.routes.get(host).is_some_and(|r| r.mesh != *mesh) {
            continue;
        }
        let ours = e
            .store
            .name(domain, name)
            .await?
            .is_some_and(|r| r.held() && r.owner.as_deref() == Some(mesh_s.as_str()));
        if ours {
            evictable.push(host);
        }
    }
    Ok(e.drop_routes(|h, r| r.mesh != *mesh && evictable.contains(&h))
        .into_iter()
        .map(|(h, _)| h)
        .collect())
}

/// `GET /v1/mesh`: the edge's current id for the session's mesh (after a rekey, the end of the chain), its
/// engines, routes, revoke list and rekey history (§5.7 `confirming`).
async fn mesh_status(Extension(c): Extension<Ctx>) -> ApiResult {
    let s = c.session()?;
    let e = &c.edge;
    let mesh = e.store.current_mesh(&s.mesh.to_string()).await?;
    let current: PublicKey = mesh.parse().map_err(|_| anyhow::anyhow!("bad mesh id {mesh}"))?;
    let attached: Vec<PublicKey> = e.mesh_tunnels(&current).iter().map(|t| t.engine).collect();
    let rows = e.store.engines(&mesh).await?;
    let revoked = rows
        .iter()
        .filter(|r| r.revoked_ms.is_some())
        .filter_map(|r| r.engine.parse().ok())
        .collect();
    let engines = rows
        .into_iter()
        .filter_map(|r| {
            let engine: PublicKey = r.engine.parse().ok()?;
            Some(MeshEngine {
                engine,
                attached: attached.contains(&engine),
                last_ms: u64::try_from(r.last_ms).unwrap_or(0),
                revoked_ms: r.revoked_ms.and_then(|v| u64::try_from(v).ok()),
            })
        })
        .collect();
    let rekeys = e
        .store
        .rekey_history(&mesh)
        .await?
        .into_iter()
        .filter_map(|(old, new, ms)| {
            Some(RekeyEntry {
                old: old.parse().ok()?,
                new: new.parse().ok()?,
                ms: u64::try_from(ms).ok()?,
            })
        })
        .collect();
    json(
        StatusCode::OK,
        &MeshStatus {
            mesh: current,
            engines,
            routes: e.mesh_routes(&current),
            stop_all_ms: stop_all_of(e, &current).await?,
            revoked,
            rekeys,
        },
    )
}

/// `POST /v1/mesh/stop`: Stop all; never refused to a delegated engine.
async fn mesh_stop(Extension(c): Extension<Ctx>) -> ApiResult {
    let s = c.session()?;
    let e = &c.edge;
    let now = now_ms();
    e.store.stop_all(&s.mesh.to_string(), now).await?;
    e.drop_routes(|_, r| r.mesh == s.mesh);
    for t in e.mesh_tunnels(&s.mesh) {
        t.kill(None, KillReason::StopAll);
    }
    json(StatusCode::OK, &StopAll { stop_all_ms: now })
}

/// `POST /v1/mesh/revoke {engines}`: append to the revoke list and close those engines' tunnels (§5.7).
async fn mesh_revoke(Extension(c): Extension<Ctx>, body: Bytes) -> ApiResult {
    let s = c.session()?;
    let e = &c.edge;
    let req: RevokeRequest = parse(&body)?;
    let cap = crate::state::REVOKED_PER_MESH;
    if req.engines.len() as u64 > cap {
        return Err(ErrorBody::invalid("engines").into());
    }
    let ids: Vec<String> = req.engines.iter().map(ToString::to_string).collect();
    // FIX4-EDGE #7, FIX5-EDGE R1/R6: unknown ids are stored revoked; at most `REVOKED_PER_MESH` revocations per
    // mesh within `REVOKE_WINDOW_MS`.
    let now = now_ms();
    let since = now.saturating_sub(crate::state::REVOKE_WINDOW_MS);
    let Some(done) = e.store.revoke(&s.mesh.to_string(), &ids, cap, since, now).await? else {
        return Err(ErrorBody::limited(ErrorCode::Quota, LimitScope::Mesh, LimitKind::Engines).into());
    };
    let revoked: Vec<PublicKey> = req
        .engines
        .into_iter()
        .filter(|k| done.contains(&k.to_string()))
        .collect();
    for t in e.mesh_tunnels(&s.mesh) {
        if revoked.contains(&t.engine) {
            e.detach(&t);
            t.kill(None, KillReason::Revoked);
        }
    }
    json(StatusCode::OK, &RevokeResult { revoked })
}

/// `POST /v1/mesh/rekey` (§5.7): a `Rekey` signed by the old and the new mesh key. Answered with or without a
/// `hello` on the connection: the two signatures are the authority, and an engine whose mesh id was already
/// rekeyed away (the heal rule's sender) cannot `hello` under it. First valid rekey wins: a second for the same
/// `old` gets `409 mesh_rekeyed {to}` with the end of the chain, which a holder whose stash holds `to` heals
/// with [`Rekey::chain`]. Applied atomically by [`crate::store::Store::rekey`]; then every tunnel under `old`
/// gets `kill {mesh_rekeyed}` (its routes go now; engines re-`hello` under `new`), and a tunnel already under
/// `new` whose engine the new revoke list names gets `kill {revoked}`.
async fn mesh_rekey(Extension(c): Extension<Ctx>, body: Bytes) -> ApiResult {
    let e = &c.edge;
    let signed: SignedRekey = parse(&body)?;
    let ident = EdgeIdentity {
        edge: e.id,
        domain: e.cfg.domain.clone(),
    };
    let r = Rekey::verify(&signed, &ident).map_err(|re| {
        let mut b = ErrorBody::new(re.code());
        b.message = Some(re.to_string());
        ApiError::from(b)
    })?;
    let revoke: Vec<String> = r.revoke.iter().map(ToString::to_string).collect();
    match e
        .store
        .rekey(&r.old.to_string(), &r.new.to_string(), &revoke, now_ms())
        .await?
    {
        Rekeyed::Applied => {}
        Rekeyed::Moved(to) => {
            let to: PublicKey = to.parse().map_err(|_| anyhow::anyhow!("bad rekeyed_to"))?;
            return Err(ErrorBody::rekeyed(to).into());
        }
        Rekeyed::Unknown => return Err(err(ErrorCode::NotFound)),
        Rekeyed::Banned => return Err(err(ErrorCode::Banned)),
        Rekeyed::NewInUse => return Err(ErrorBody::invalid("new").into()),
    }
    e.drop_routes(|_, route| route.mesh == r.old);
    for t in e.mesh_tunnels(&r.old) {
        t.kill(None, KillReason::MeshRekeyed);
    }
    for t in e.mesh_tunnels(&r.new) {
        if r.revoke.contains(&t.engine) {
            e.detach(&t);
            t.kill(None, KillReason::Revoked);
        }
    }
    json(StatusCode::OK, &RekeyAccepted { mesh: r.new })
}
