//! `theuth/share/1` (210 §5.3): the protocol handler that admits engine connections, and the control API, one
//! HTTP/1.1 request per bi-stream the engine opens, served by axum through hyper as a2a's `serve_http` does.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Extension, Path};
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
use share_proto::version::{client_ok, edge_agent};
use share_proto::{
    Admission, Delegation, EdgeIdentity, ErrorBody, ErrorCode, Hello, Info, InterstitialRule,
    KillReason, MeshEngine, MeshStatus, NameAvailability, NameGranted, NameInfo, NameKind,
    NameReleased, NameRequest, NameState, PROTOCOL_MAJOR, PublicKey, RevokeRequest, RevokeResult,
    RouteEntry, RoutesPut, SpkiHash, StopAll, Welcome, WelcomeLimits, paths,
};
use tower::ServiceExt as _;

use crate::state::{Edge, Route, Session, Tunnel, now_ms};
use crate::store::{NameRow, NewName};

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
        let tunnel = Tunnel::new(conn, Some(permit));
        serve(edge, tunnel).await;
        Ok(())
    }
}

/// The life of one engine connection: control streams until close, the `hello` deadline and delegation expiry.
async fn serve(edge: Arc<Edge>, tunnel: Arc<Tunnel>) {
    let api = api();
    let hello_deadline =
        tokio::time::Instant::now() + Duration::from_secs(edge.cfg.limits.hello_timeout_secs);
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
                    // §5.7: at expiry without a renewed delegation.
                    tunnel.kill(None, KillReason::DelegationExpired);
                    edge.detach(&tunnel);
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
            if req.uri().path() == paths::EVENTS {
                flag.store(true, Ordering::Relaxed);
            }
            req.extensions_mut().insert(Ctx { edge, tunnel });
            Ok::<_, Infallible>(api.oneshot(req).await.unwrap_or_else(|e| match e {}))
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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.1.unwrap_or_else(|| {
            StatusCode::from_u16(self.0.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        });
        (status, axum::Json(self.0)).into_response()
    }
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

/// `GET /v1/info`, the one request answered before `hello`. TODO(210-M3): per-IP rate limit (`info_per_ip_min`).
async fn info(Extension(c): Extension<Ctx>) -> ApiResult {
    let e = &c.edge;
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
            terms_url: e.cfg.terms_url.clone(),
            terms_rev: e.cfg.terms_rev.clone(),
            abuse_url: e.cfg.abuse_url.clone(),
            // TODO(210-M3): `names_max` at slot time.
            full: false,
        },
    )
}

/// `POST /v1/hello` (§5.7): delegation check, bans, rekey, revoke, admission, engine count; then `welcome`.
async fn hello(Extension(c): Extension<Ctx>, body: Bytes) -> ApiResult {
    let e = c.edge.clone();
    let h: Hello = parse(&body)?;
    if !client_ok(&h.client, &e.cfg.min_client) {
        return Err(err(ErrorCode::ClientTooOld));
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
    if e.paused.load(Ordering::Relaxed) {
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
    if let Some(to) = known.as_ref().and_then(|m| m.rekeyed_to.as_deref()) {
        let to: PublicKey = to.parse().map_err(|_| anyhow::anyhow!("bad rekeyed_to"))?;
        return Err(ErrorBody::rekeyed(to).into());
    }
    if e.store.revoked(&mesh, &engine).await?.is_some() {
        return Err(err(ErrorCode::Revoked));
    }
    let mut invited = false;
    if known.is_none() {
        match e.admission() {
            Admission::Open => {}
            Admission::Closed => return Err(err(ErrorCode::AdmissionClosed)),
            Admission::Invite => {
                let ok = match &h.invite {
                    Some(inv) => e.store.consume_invite(&inv.hash(), now).await?,
                    None => false,
                };
                if !ok {
                    return Err(err(ErrorCode::InviteRequired));
                }
                invited = true;
            }
        }
        // TODO(210-P2): `allowlist` admission. TODO(210-M3): per-network new-mesh limits, relayed newcomers.
    }
    let others = e
        .mesh_tunnels(&d.mesh)
        .iter()
        .filter(|t| t.engine != d.engine)
        .count();
    if others >= e.cfg.engines_per_mesh as usize {
        return Err(err(ErrorCode::EnginesQuota));
    }
    e.store
        .attach(&mesh, &engine, &c.tunnel.net, invited, now)
        .await?;
    // An edge without terms needs no acceptance.
    let terms_ok =
        e.cfg.terms_rev.is_empty() || h.terms_rev.as_deref() == Some(e.cfg.terms_rev.as_str());
    *c.tunnel.session.lock().expect("session lock") = Some(Session {
        mesh: d.mesh,
        expires_ms: d.expires_ms,
        terms_ok,
    });
    c.tunnel.pre_hello.lock().expect("permit lock").take();
    // §5.3: one tunnel per engine key; a new one replaces the old.
    if let Some(old) = e.tunnels.insert(d.engine, c.tunnel.clone())
        && !Arc::ptr_eq(&old, &c.tunnel)
    {
        e.detach(&old);
        old.conn.close(6u32.into(), b"replaced");
    }
    let names = e
        .store
        .held_names(c.domain(), &mesh)
        .await?
        .iter()
        .map(name_info)
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
                routes_per_tunnel: e.cfg.limits.routes_per_tunnel,
                conns_per_tunnel: e.cfg.limits.conns_per_tunnel,
                delegation_max_ttl_ms: max_ttl,
                ..WelcomeLimits::default()
            },
            terms_url: e.cfg.terms_url.clone(),
            abuse_url: e.cfg.abuse_url.clone(),
            terms_rev: e.cfg.terms_rev.clone(),
            terms_ok,
            full: false,
            // TODO(210-M4): `retire` when running on the standby key.
            retire: None,
        },
    )
}

fn name_info(r: &NameRow) -> NameInfo {
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
        // TODO(210-M3): the slot queue position.
        queue: None,
    }
}

/// `GET /v1/names/{name}`. TODO(210-M3): per-network rate limit.
async fn availability(Extension(c): Extension<Ctx>, Path(name): Path<String>) -> ApiResult {
    let s = c.session()?;
    let reason = |r: &str| NameAvailability {
        available: false,
        reason: Some(r.into()),
    };
    if names::check_random(&name).is_err()
        && let Err(e) = names::check_reserved(&name, &c.edge.cfg.reserved_words)
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
        },
        Some(r) if r.state == "banned" => reason("banned"),
        Some(r) if r.owner.as_deref() == Some(mesh.as_str()) => NameAvailability {
            available: true,
            reason: None,
        },
        Some(r) if r.held() => reason("taken"),
        Some(r) if r.reclaim_after_ms.is_some_and(|t| t > now_i64()) => reason("cooldown"),
        Some(_) => NameAvailability {
            available: true,
            reason: None,
        },
    };
    json(StatusCode::OK, &answer)
}

fn now_i64() -> i64 {
    i64::try_from(now_ms()).unwrap_or(i64::MAX)
}

/// The claim rules of §5.2 shared by random allocation and reserved claims. `Ok(true)` = already held by this
/// mesh (idempotent).
async fn claim(c: &Ctx, s: &Session, name: &str, kind: NameKind) -> Result<bool, ApiError> {
    if !s.terms_ok {
        return Err(err(ErrorCode::TermsRequired));
    }
    let e = &c.edge;
    if e.store.ban_reason("name", name).await?.is_some() {
        return Err(err(ErrorCode::Banned));
    }
    let mesh = s.mesh.to_string();
    if let Some(r) = e.store.name(c.domain(), name).await? {
        let mine = r.owner.as_deref() == Some(mesh.as_str());
        if r.state == "banned" {
            return Err(err(ErrorCode::Banned));
        }
        if r.held() {
            return if mine {
                Ok(true)
            } else {
                Err(err(ErrorCode::NameTaken))
            };
        }
        // Released: until the cooldown ends only the previous owner may claim it again (§5.2).
        if let Some(t) = r.reclaim_after_ms
            && t > now_i64()
            && !mine
        {
            let mut b = ErrorBody::new(ErrorCode::Cooldown);
            b.reclaim_after_ms = u64::try_from(t).ok();
            return Err(b.into());
        }
    }
    let held = e.store.held_names(c.domain(), &mesh).await?;
    let reserved = held.iter().filter(|r| r.kind == "reserved").count();
    let over = match kind {
        // One random name, only while the mesh holds no name at all.
        NameKind::Random => !held.is_empty(),
        NameKind::Reserved => reserved >= e.cfg.reserved_per_mesh as usize,
    };
    if over {
        return Err(err(ErrorCode::Quota));
    }
    // TODO(210-M3): paced bucket (`429 paced`), `edge_full`, per-network claim and quota limits.
    e.store
        .claim(NewName {
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
        })
        .await?;
    Ok(false)
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
    names::check_reserved(&name, &c.edge.cfg.reserved_words)
        .and_then(|()| names::check_brand(&name, &c.edge.brands))
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
    let Some(r) = e.store.name(c.domain(), &name).await? else {
        return Err(err(ErrorCode::NotFound));
    };
    if !r.held() || r.owner.as_deref() != Some(mesh.as_str()) {
        return Err(err(ErrorCode::NotOwner));
    }
    let now = now_ms();
    let cooldown = u64::try_from(e.cfg.reclaim_cooldown.as_millis()).unwrap_or(u64::MAX);
    let not_after = r
        .not_after_ms
        .and_then(|v| u64::try_from(v).ok())
        .unwrap_or(0);
    let reclaim_after_ms = now.saturating_add(cooldown).max(not_after);
    e.store
        .release(c.domain(), &name, now, reclaim_after_ms)
        .await?;
    e.drop_routes(|_, route| route.name == name && route.mesh == s.mesh);
    // TODO(EDGE2): delete the name's DNS records, revoke its certificate, drop a queued job.
    json(
        StatusCode::OK,
        &NameReleased {
            name,
            reclaim_after_ms,
        },
    )
}

/// `POST /v1/names/{name}/transfer`. TODO(210-M2).
async fn transfer(Extension(c): Extension<Ctx>) -> ApiResult {
    c.session()?;
    Err(not_yet("name transfer"))
}

/// `POST /v1/names/{name}/csr`. TODO(EDGE2): CSR check, job, chain.
async fn csr(Extension(c): Extension<Ctx>) -> ApiResult {
    c.session()?;
    Err(not_yet("certificate issuance"))
}

/// `GET /v1/names/{name}/cert`. TODO(EDGE2).
async fn cert(Extension(c): Extension<Ctx>) -> ApiResult {
    c.session()?;
    Err(not_yet("certificate issuance"))
}

/// `PUT /v1/routes {hosts}`: this engine's full set; all or nothing (§5.4).
async fn put_routes(Extension(c): Extension<Ctx>, body: Bytes) -> ApiResult {
    let s = c.session()?;
    let e = &c.edge;
    let req: RoutesPut = parse(&body)?;
    if req.hosts.len() > e.cfg.limits.routes_per_tunnel as usize {
        return Err(err(ErrorCode::Quota));
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
        let prev = e.routes.insert(
            host.clone(),
            Route {
                name,
                mesh: s.mesh,
                tunnel: tunnel.clone(),
            },
        );
        if prev.is_none_or(|p| !Arc::ptr_eq(&p.tunnel, &tunnel)) {
            e.announce(&s.mesh, &host, Some(tunnel.engine));
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /v1/events`: SSE held for the connection. TODO(210-M3): `stats` every 30 s from the metering.
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

/// `GET /v1/mesh`.
async fn mesh_status(Extension(c): Extension<Ctx>) -> ApiResult {
    let s = c.session()?;
    let e = &c.edge;
    let mesh = s.mesh.to_string();
    let attached: Vec<PublicKey> = e.mesh_tunnels(&s.mesh).iter().map(|t| t.engine).collect();
    let engines = e
        .store
        .engines(&mesh)
        .await?
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
    let stop_all_ms = e
        .store
        .mesh(&mesh)
        .await?
        .and_then(|m| m.stop_all_ms)
        .and_then(|v| u64::try_from(v).ok());
    json(
        StatusCode::OK,
        &MeshStatus {
            mesh: s.mesh,
            engines,
            routes: e.mesh_routes(&s.mesh),
            stop_all_ms,
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
    let ids: Vec<String> = req.engines.iter().map(ToString::to_string).collect();
    e.store.revoke(&s.mesh.to_string(), &ids, now_ms()).await?;
    for t in e.mesh_tunnels(&s.mesh) {
        if req.engines.contains(&t.engine) {
            e.detach(&t);
            t.kill(None, KillReason::Revoked);
        }
    }
    json(
        StatusCode::OK,
        &RevokeResult {
            revoked: req.engines,
        },
    )
}

/// `POST /v1/mesh/rekey`. TODO(210-M2): `Rekey::verify`, the atomic move of names, invites and routes.
async fn mesh_rekey(Extension(c): Extension<Ctx>) -> ApiResult {
    c.session()?;
    Err(not_yet("mesh rekey"))
}
