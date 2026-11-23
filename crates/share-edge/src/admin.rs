//! The admin socket (210 §8.5): newline-delimited `AdminRequest`/`AdminResponse` JSON on a 0600 Unix socket,
//! peer uid checked against the edge's own; and the client the `admin` subcommand uses.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use serde_json::{Value, json};
use share_proto::admin::{AdminCommand, AdminRequest, AdminResponse, BanKind, ListWhat};
use share_proto::{ErrorCode, KillReason};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::state::{Edge, now_ms};

fn kind_str(k: BanKind) -> &'static str {
    match k {
        BanKind::Name => "name",
        BanKind::Mesh => "mesh",
        BanKind::Key => "key",
        BanKind::Net => "net",
    }
}

/// Bind the socket (replacing a stale file), mode 0600.
pub fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("remove stale {}", path.display()))?;
    }
    let l = UnixListener::bind(path).with_context(|| format!("bind {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// Serve admin connections until the listener fails.
pub async fn serve(edge: Arc<Edge>, listener: UnixListener, socket: std::path::PathBuf) {
    // The socket file is created by this process, so its owner is the edge's uid (no `unsafe` geteuid).
    let own_uid = std::fs::metadata(&socket).map(|m| m.uid()).ok();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let peer_uid = stream.peer_cred().ok().map(|c| c.uid());
        if own_uid.is_none() || peer_uid != own_uid {
            tracing::warn!(?peer_uid, "admin socket: refused a peer with another uid");
            continue;
        }
        tokio::spawn(connection(edge.clone(), stream));
    }
}

async fn connection(edge: Arc<Edge>, stream: UnixStream) {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<AdminRequest>(&line) {
            Ok(req) => handle(&edge, req).await,
            Err(e) => AdminResponse::err(0, ErrorCode::Invalid, e.to_string()),
        };
        let mut out = serde_json::to_vec(&resp).unwrap_or_default();
        out.push(b'\n');
        if w.write_all(&out).await.is_err() {
            break;
        }
    }
}

/// Answer one request.
pub async fn handle(edge: &Arc<Edge>, req: AdminRequest) -> AdminResponse {
    let id = req.id;
    if req.command.is_mutating() {
        let args = serde_json::to_value(&req.command)
            .map(|v| v.to_string())
            .unwrap_or_default();
        let cmd = args_cmd(&req.command);
        if let Err(e) = edge
            .store
            .audit(&cmd, &args, req.reference.as_deref(), now_ms())
            .await
        {
            return AdminResponse::err(id, ErrorCode::Internal, format!("audit: {e:#}"));
        }
    }
    match run(edge, req.command).await {
        Ok(v) => AdminResponse::ok(id, v),
        Err((code, msg)) => AdminResponse::err(id, code, msg),
    }
}

fn args_cmd(c: &AdminCommand) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.get("cmd").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default()
}

type AdminResult = Result<Value, (ErrorCode, String)>;

fn internal(e: anyhow::Error) -> (ErrorCode, String) {
    (ErrorCode::Internal, format!("{e:#}"))
}

async fn run(edge: &Arc<Edge>, cmd: AdminCommand) -> AdminResult {
    match cmd {
        AdminCommand::Status => Ok(json!({
            "version": crate::state::VERSION,
            "domain": edge.cfg.domain,
            "edge_id": edge.id.to_string(),
            "admission": edge.admission(),
            "paused": edge.paused.load(Ordering::Relaxed),
            "tunnels": edge.tunnels.len(),
            "routes": edge.routes.len(),
            "visitors": edge.cfg.limits.global_conns as usize - edge.visitors.available_permits(),
        })),
        AdminCommand::List { what } => list(edge, what.unwrap_or(ListWhat::Names)).await,
        AdminCommand::Ban {
            kind,
            value,
            reason,
        } => {
            let k = kind_str(kind);
            edge.store
                .ban(k, &value, &reason, now_ms())
                .await
                .map_err(internal)?;
            let closed = apply_ban(edge, kind, &value);
            Ok(json!({ "closed_tunnels": closed }))
        }
        AdminCommand::Unban { kind, value } => {
            if edge
                .store
                .unban(kind_str(kind), &value)
                .await
                .map_err(internal)?
            {
                Ok(json!({}))
            } else {
                Err((
                    ErrorCode::NotFound,
                    format!("no {} ban on {value}", kind_str(kind)),
                ))
            }
        }
        AdminCommand::Admission { mode } => {
            // Live only: a restart returns to the config's `admission`.
            *edge.admission.write().expect("admission lock") = mode;
            Ok(json!({ "admission": mode }))
        }
        AdminCommand::Pause => {
            edge.paused.store(true, Ordering::Relaxed);
            let n = edge.tunnels.len();
            for t in edge
                .tunnels
                .iter()
                .map(|t| t.value().clone())
                .collect::<Vec<_>>()
            {
                edge.detach(&t);
                t.conn.close(2u32.into(), b"paused");
            }
            Ok(json!({ "closed_tunnels": n }))
        }
        AdminCommand::Resume => {
            edge.paused.store(false, Ordering::Relaxed);
            Ok(json!({}))
        }
        // TODO(210-M4): queue, mesh, release, evict-unpublished, recover, invite new, caa-check, revoke-cert,
        // reload, brake-status.
        other => Err((
            ErrorCode::Invalid,
            format!("`{}` is not available on this edge yet", args_cmd(&other)),
        )),
    }
}

async fn list(edge: &Arc<Edge>, what: ListWhat) -> AdminResult {
    Ok(match what {
        ListWhat::Names => Value::Array(
            edge.store
                .all_names()
                .await
                .map_err(internal)?
                .into_iter()
                .map(|n| json!({"name": n.name, "kind": n.kind, "state": n.state, "owner": n.owner}))
                .collect(),
        ),
        ListWhat::Meshes => Value::Array(
            edge.store
                .all_meshes()
                .await
                .map_err(internal)?
                .into_iter()
                .map(|m| json!({"mesh": m.mesh, "invited": m.invited != 0, "rekeyed_to": m.rekeyed_to}))
                .collect(),
        ),
        ListWhat::Tunnels => Value::Array(
            edge.tunnels
                .iter()
                .map(|t| {
                    let routes: Vec<String> = edge
                        .routes
                        .iter()
                        .filter(|r| Arc::ptr_eq(&r.tunnel, t.value()))
                        .map(|r| r.key().clone())
                        .collect();
                    json!({
                        "engine": t.engine.to_string(),
                        "mesh": t.mesh().map(|m| m.to_string()),
                        "net": t.net,
                        "streams": t.streams.load(Ordering::Relaxed),
                        "conns": t.conns.load(Ordering::Relaxed),
                        "bytes": t.bytes.load(Ordering::Relaxed),
                        "routes": routes,
                    })
                })
                .collect(),
        ),
    })
}

/// §9.3: close the affected tunnels with `kill {edge_ban}` and drop their routes. Returns the tunnels touched.
fn apply_ban(edge: &Arc<Edge>, kind: BanKind, value: &str) -> usize {
    match kind {
        BanKind::Name => {
            let removed = edge.drop_routes(|_, r| r.name == value);
            let mut touched: Vec<_> = Vec::new();
            for (host, r) in removed {
                r.tunnel.kill(Some(host), KillReason::EdgeBan);
                if !touched.iter().any(|t| Arc::ptr_eq(t, &r.tunnel)) {
                    touched.push(r.tunnel);
                }
            }
            touched.len()
        }
        BanKind::Mesh | BanKind::Key | BanKind::Net => {
            let hit: Vec<_> = edge
                .tunnels
                .iter()
                .filter(|t| match kind {
                    BanKind::Mesh => t.mesh().is_some_and(|m| m.to_string() == value),
                    BanKind::Key => t.engine.to_string() == value,
                    _ => t.net == value,
                })
                .map(|t| t.value().clone())
                .collect();
            for t in &hit {
                edge.detach(t);
                t.kill(None, KillReason::EdgeBan);
            }
            hit.len()
        }
    }
}

/// Send one request over the socket and return the answer.
pub async fn request(socket: &Path, req: &AdminRequest) -> anyhow::Result<AdminResponse> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let (r, mut w) = stream.into_split();
    let mut line = serde_json::to_vec(req)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    let mut lines = BufReader::new(r).lines();
    let answer = lines.next_line().await?.context("admin socket closed")?;
    Ok(serde_json::from_str(&answer)?)
}
