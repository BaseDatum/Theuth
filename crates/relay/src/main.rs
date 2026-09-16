//! `wth-relay`: a stateless relay that maps a harness id to one harness
//! WebSocket and copies opaque encrypted frames between it and that
//! harness's devices (design doc 31 §5).

mod cookie;
mod frame;
mod relay;
mod web;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, bail};
use axum::Router;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum_server::Handle;
use axum_server::accept::NoDelayAcceptor;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use base64::Engine;
use clap::Parser;

use crate::relay::Relay;

/// `wth-relay --listen 0.0.0.0:8787 [--tls <cert.pem> <key.pem>] [--web <dir>] [--max-conns 64] [--cookie-secret <hex>]`
#[derive(Parser, Debug)]
#[command(
    name = "wth-relay",
    version,
    about = "Stateless encrypted-frame relay between wth harnesses and devices"
)]
struct Cli {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:8787")]
    listen: SocketAddr,
    /// Terminate TLS in-process: certificate chain and private key PEM files.
    #[arg(long, num_args = 2, value_names = ["CERT", "KEY"])]
    tls: Option<Vec<PathBuf>>,
    /// Serve this directory (the UI bundle) at `/`, `/link` and `/assets/*`.
    #[arg(long)]
    web: Option<PathBuf>,
    /// Device conns per harness before `CLOSE 3`.
    #[arg(long, default_value_t = 64)]
    max_conns: usize,
    /// Hex secret for the browser cookie HMAC; empty or absent → random per start.
    #[arg(long)]
    cookie_secret: Option<String>,
}

/// Router state.
#[derive(Clone)]
pub struct App {
    relay: Arc<Relay>,
    web: Option<Arc<web::Web>>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let tls = match cli.tls.as_deref() {
        None => None,
        Some([cert, key]) => {
            if rustls::crypto::CryptoProvider::get_default().is_none() {
                let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            }
            Some(
                RustlsConfig::from_pem_file(cert, key)
                    .await
                    .with_context(|| format!("--tls {} {}", cert.display(), key.display()))?,
            )
        }
        Some(_) => bail!("--tls takes exactly <cert.pem> <key.pem>"),
    };
    let secret = match cli.cookie_secret.as_deref() {
        None | Some("") => None,
        Some(hex) => Some(cookie::unhex(hex).context("--cookie-secret must be hex")?),
    };
    let web = cli.web.as_deref().map(|dir| {
        Arc::new(web::Web::new(
            dir,
            cookie::Signer::new(secret),
            tls.is_some(),
        ))
    });
    let relay = Arc::new(Relay::new(cli.max_conns));
    let app = App {
        relay: relay.clone(),
        web: web.clone(),
    };

    let mut router = Router::new()
        .route("/h/{id}", get(socket))
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(metrics));
    if web.is_some() {
        router = router.fallback(web::pages);
    }
    let svc = router
        .with_state(app)
        .into_make_service_with_connect_info::<SocketAddr>();

    // TCP_NODELAY: a relayed frame must not wait for Nagle behind a previous small write.
    let handle = Handle::new();
    let server = axum_server::bind(cli.listen).handle(handle.clone());
    let mut server = match tls {
        Some(cfg) => tokio::spawn(
            server
                .acceptor(RustlsAcceptor::new(cfg).acceptor(NoDelayAcceptor))
                .serve(svc),
        ),
        None => tokio::spawn(server.acceptor(NoDelayAcceptor).serve(svc)),
    };
    tracing::info!(listen = %cli.listen, tls = cli.tls.is_some(), web = ?cli.web, max_conns = cli.max_conns, "wth-relay up");

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
        r = &mut server => return r?.context("serve"),
    }
    tracing::info!("shutting down");
    handle.graceful_shutdown(Some(Duration::from_secs(5)));
    relay.stop();
    let drain = tokio::time::Instant::now() + Duration::from_secs(3);
    while relay.metrics.sockets.load(Ordering::Relaxed) > 0 && tokio::time::Instant::now() < drain {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    server.await?.context("serve")
}

async fn metrics(State(app): State<App>) -> String {
    app.relay.render_metrics()
}

/// `/h/<id>`: device socket, or the harness uplink with `?role=harness`.
async fn socket(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(id) = decode_id(&id) else {
        return (StatusCode::BAD_REQUEST, "bad harness id").into_response();
    };
    if !app.relay.allow(client_ip(&headers, addr)) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if let Some(web) = app.web.as_deref()
        && !web::same_origin_ok(web, &headers)
    {
        app.relay
            .metrics
            .cookie_rejected
            .fetch_add(1, Ordering::Relaxed);
        return StatusCode::FORBIDDEN.into_response();
    }
    let ws = ws
        .protocols([relay::PROTOCOL])
        .max_message_size(frame::MAX_MESSAGE)
        .max_frame_size(frame::MAX_MESSAGE)
        .read_buffer_size(4096);
    if ws.selected_protocol().is_none() {
        return (
            StatusCode::BAD_REQUEST,
            "Sec-WebSocket-Protocol: wth-link-1 required",
        )
            .into_response();
    }
    let harness = query
        .as_deref()
        .is_some_and(|q| q.split('&').any(|kv| kv == "role=harness"));
    let relay = app.relay.clone();
    ws.on_upgrade(move |sock| async move {
        if harness {
            relay::uplink(relay, id, sock).await
        } else {
            relay::device(relay, id, sock).await
        }
    })
}

/// base64url (no pad) of exactly 32 bytes.
fn decode_id(s: &str) -> Option<[u8; 32]> {
    let v = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()?;
    v.try_into().ok()
}

/// First `X-Forwarded-For` hop when present, else the peer address.
fn client_ip(h: &HeaderMap, addr: SocketAddr) -> IpAddr {
    h.get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(addr.ip())
}
