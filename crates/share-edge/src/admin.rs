//! The admin socket (210 §8.5): newline-delimited `AdminRequest`/`AdminResponse` JSON on a 0600 Unix socket,
//! peer uid checked against the edge's own; and the client the `admin` subcommand uses.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use serde_json::{Value, json};
use share_proto::admin::{
    AdminCommand, AdminRequest, AdminResponse, AdmissionResult, BanInfo, BanKind, CaaCheckResult, CertRetryResult,
    ClosedTunnels, EvictResult, InviteListResult, InviteMinted, InviteRevoked, InviteRow, JobKind, ListWhat,
    MeshResult, QueueClass, QueueResult, QueueRow, RecoverResult, ReleaseResult, ReloadResult, RevokeCertResult,
    RevokeReason, StatusResult,
};
use share_proto::{Admission, ErrorCode, Invite, KillReason, MeshEngine, PublicKey, RekeyEntry};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::state::{Edge, Live, now_ms};
use crate::store::{InviteRevoke, hex_lower};

fn kind_str(k: BanKind) -> &'static str {
    match k {
        BanKind::Name => "name",
        BanKind::Mesh => "mesh",
        BanKind::Key => "key",
        BanKind::Net => "net",
    }
}

/// Longest request line the socket reads (m1); a longer one is answered `too_large` and the connection closed.
pub const MAX_LINE: usize = share_proto::limits::MAX_JSON_BODY;

/// Bind the socket (replacing a stale file), mode 0600. The socket is created inside a fresh 0700 directory
/// next to `path`, set to 0600 there, then renamed into place (m2): it is never reachable with the umask's
/// permissions.
pub fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    use std::os::unix::fs::DirBuilderExt as _;
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("remove stale {}", path.display()))?;
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    // Short names: the staging path must fit SUN_LEN (104 on macOS) wherever the final socket does.
    let staging = parent.join(format!(".ab{:08x}", rand::random::<u32>()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&staging)
        .with_context(|| format!("create {}", staging.display()))?;
    let tmp = staging.join("s");
    let bound = bind_private(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_dir(&staging);
    bound
}

fn bind_private(tmp: &Path, path: &Path) -> anyhow::Result<UnixListener> {
    let l = UnixListener::bind(tmp).with_context(|| format!("bind {}", tmp.display()))?;
    std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(tmp, path).with_context(|| format!("move socket to {}", path.display()))?;
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
    let mut reader = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let limit = u64::try_from(MAX_LINE + 1).unwrap_or(u64::MAX);
        match (&mut reader).take(limit).read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let too_long = buf.last() != Some(&b'\n') && buf.len() > MAX_LINE;
        let resp = if too_long {
            AdminResponse::err(0, ErrorCode::TooLarge, format!("request line over {MAX_LINE} bytes"))
        } else {
            let line = String::from_utf8_lossy(&buf);
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<AdminRequest>(&line) {
                Ok(req) => handle(&edge, req).await,
                Err(e) => AdminResponse::err(0, ErrorCode::Invalid, e.to_string()),
            }
        };
        let mut out = serde_json::to_vec(&resp).unwrap_or_default();
        out.push(b'\n');
        if w.write_all(&out).await.is_err() || too_long {
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
    let now = now_ms();
    match cmd {
        AdminCommand::Status => typed(&status(edge, now).await.map_err(internal)?),
        AdminCommand::List { what } => list(edge, what.unwrap_or(ListWhat::Names)).await,
        AdminCommand::Ban {
            kind,
            value,
            reason,
        } => {
            let k = kind_str(kind);
            // FIX4-EDGE #5: a name ban's value reaches DNS and k8s deletes; only a valid label may.
            if kind == BanKind::Name {
                share_proto::names::check_label(&value)
                    .map_err(|e| (ErrorCode::Invalid, format!("name: {}", e.reason())))?;
            }
            edge.store
                .ban(&edge.cfg.domain, k, &value, &reason, now)
                .await
                .map_err(internal)?;
            let closed = apply_ban(edge, kind, &value);
            typed(&ClosedTunnels { closed_tunnels: closed as u64 })
        }
        AdminCommand::Unban { kind, value } => {
            if edge
                .store
                .unban(&edge.cfg.domain, kind_str(kind), &value)
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
            typed(&AdmissionResult { admission: mode })
        }
        AdminCommand::Pause => {
            edge.store.set_paused(true).await.map_err(internal)?;
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
            typed(&ClosedTunnels { closed_tunnels: n as u64 })
        }
        AdminCommand::Resume => {
            edge.store.set_paused(false).await.map_err(internal)?;
            edge.paused.store(false, Ordering::Relaxed);
            Ok(json!({}))
        }
        AdminCommand::Queue => typed(&queue(edge, now).await.map_err(internal)?),
        AdminCommand::Mesh { id } => mesh(edge, id).await,
        AdminCommand::Release { name } => release(edge, &name, now).await,
        AdminCommand::EvictUnpublished => {
            let after = u64::try_from(edge.live().limits.evict_unpublished_after.as_millis()).unwrap_or(u64::MAX);
            let names = edge
                .store
                .unpublished_names(&edge.cfg.domain, now.saturating_sub(after))
                .await
                .map_err(internal)?;
            let mut evicted = Vec::new();
            for n in names {
                if crate::queue::evict(edge, &n, true, now).await.map_err(internal)? {
                    evicted.push(n);
                }
            }
            typed(&EvictResult { evicted })
        }
        AdminCommand::Recover { target, to_mesh } => recover(edge, &target, to_mesh, now).await,
        AdminCommand::InviteNew {
            uses,
            expires_s,
            note,
        } => {
            let uses = uses.unwrap_or(1).max(1);
            let expires_ms = now.saturating_add(expires_s.unwrap_or(7 * 86_400).saturating_mul(1000));
            let invite = Invite(rand::random::<[u8; 16]>());
            edge.store
                .mint_invite(&invite.hash(), uses, expires_ms, note.as_deref(), now)
                .await
                .map_err(internal)?;
            let mut ticket = crate::ticket(&edge.cfg, edge.id).map_err(internal)?;
            ticket.invite = Some(invite);
            typed(&InviteMinted {
                ticket,
                hash: invite.hash_hex(),
                uses,
                expires_ms,
            })
        }
        AdminCommand::InviteList => {
            let invites = edge
                .store
                .live_invites(now)
                .await
                .map_err(internal)?
                .into_iter()
                .map(|r| InviteRow {
                    hash: hex_lower(&r.hash),
                    uses_left: u32::try_from(r.uses_left).unwrap_or(0),
                    expires_ms: u64::try_from(r.expires_ms).unwrap_or(0),
                    created_ms: u64::try_from(r.created_ms).unwrap_or(0),
                    note: r.note,
                })
                .collect();
            typed(&InviteListResult { invites })
        }
        AdminCommand::InviteRevoke { hash } => {
            let prefix = hash.trim().to_ascii_lowercase();
            if prefix.len() < 8 || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err((ErrorCode::Invalid, "hash: at least 8 hex characters".into()));
            }
            match edge.store.revoke_invite(&prefix, now).await.map_err(internal)? {
                InviteRevoke::Revoked(hash) => typed(&InviteRevoked { hash }),
                InviteRevoke::NotFound => Err((ErrorCode::NotFound, format!("no live invite {prefix}…"))),
                InviteRevoke::Ambiguous => Err((ErrorCode::Invalid, format!("{prefix}… matches several invites"))),
            }
        }
        AdminCommand::CaaCheck => typed(&caa_check(edge, now).await.map_err(internal)?),
        AdminCommand::RevokeCert { name, reason } => revoke_cert(edge, &name, reason.unwrap_or_default(), now).await,
        AdminCommand::CertRetry { name } => cert_retry(edge, &name, now).await,
        AdminCommand::Reload => {
            let Some(path) = edge.config_path.get() else {
                return Err((ErrorCode::Invalid, "this edge was not started from a config file".into()));
            };
            let (cfg, _) = crate::config::Config::load(path).map_err(internal)?;
            typed(&reload(edge, &cfg).map_err(internal)?)
        }
        AdminCommand::BrakeStatus => typed(&crate::ops::brake_status(edge, now).await.map_err(internal)?),
    }
}

fn typed<T: serde::Serialize>(v: &T) -> AdminResult {
    serde_json::to_value(v).map_err(|e| internal(e.into()))
}

/// `status`.
pub async fn status(edge: &Edge, now: u64) -> anyhow::Result<StatusResult> {
    let live = edge.live();
    let (start, end) = crate::limits::month_bounds(now);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let pace = (live.limits.egress_month as f64 * (now.saturating_sub(start) as f64 / end.saturating_sub(start).max(1) as f64))
        as u64;
    Ok(StatusResult {
        version: crate::state::VERSION.into(),
        domain: edge.cfg.domain.clone(),
        edge_id: edge.id,
        admission: edge.admission(),
        paused: edge.paused.load(Ordering::Relaxed),
        tunnels: edge.tunnels.len() as u64,
        routes: edge.routes.len() as u64,
        visitors: (edge.cfg.limits.global_conns as usize).saturating_sub(edge.visitors.available_permits()) as u64,
        full: edge.full(),
        names_slotted: edge.store.slotted_count(&edge.cfg.domain).await?,
        names_max: u64::from(live.limits.names_max),
        queued: edge.store.slot_jobs(&edge.cfg.domain).await?.len() as u64,
        queue_max: u64::from(live.queue_max),
        throttled: edge.meter.throttled.load(Ordering::Relaxed),
        egress_month_bytes: edge.meter.month_bytes.load(Ordering::Relaxed),
        egress_pace_bytes: pace,
        standby: edge.standby.get().copied(),
    })
}

/// `queue`: the jobs waiting for a slot, invited class first, in §6.5 order.
pub async fn queue(edge: &Edge, now: u64) -> anyhow::Result<QueueResult> {
    let Some(certs) = edge.certs.get() else {
        return Ok(QueueResult { invited_tokens: 0, open_tokens: 0, next_token_s: 0, jobs: Vec::new() });
    };
    let ca = certs.ca.id();
    let jobs = edge.store.slot_jobs(&edge.cfg.domain).await?;
    let mut out = QueueResult { invited_tokens: 0, open_tokens: 0, next_token_s: u64::MAX, jobs: Vec::new() };
    for (class, qc) in [("invited", QueueClass::Invited), ("open", QueueClass::Open)] {
        let v = crate::queue::view(edge, ca, class, &jobs, now).await?;
        if class == "invited" {
            out.invited_tokens = v.tokens;
        } else {
            out.open_tokens = v.tokens;
        }
        out.next_token_s = out.next_token_s.min(v.next_token_s);
        for j in crate::queue::order(&jobs, class) {
            let Ok(mesh) = j.mesh.parse() else { continue };
            let pos = v.pos(&j.id);
            out.jobs.push(QueueRow {
                name: j.name.clone(),
                mesh,
                class: qc,
                kind: JobKind::New,
                position: if j.state == "ordering" { 0 } else { pos.map_or(0, |p| p.position) },
                eta_s: pos.map_or(0, |p| p.eta_s),
                first_of_mesh: j.first_of_mesh != 0,
                net: crate::limits::wide_key(&j.net),
                enqueued_ms: u64::try_from(j.enqueued_ms).unwrap_or(0),
                attempts: u32::try_from(j.attempts).unwrap_or(0),
            });
        }
    }
    if out.next_token_s == u64::MAX {
        out.next_token_s = 0;
    }
    Ok(out)
}

async fn mesh(edge: &Arc<Edge>, id: PublicKey) -> AdminResult {
    let m = id.to_string();
    let Some(row) = edge.store.mesh(&m).await.map_err(internal)? else {
        return Err((ErrorCode::NotFound, format!("no mesh {m}")));
    };
    let detail = edge.store.mesh_detail(&m).await.map_err(internal)?.ok_or_else(|| {
        (ErrorCode::NotFound, format!("no mesh {m}"))
    })?;
    let names = edge
        .store
        .held_names(&edge.cfg.domain, &m)
        .await
        .map_err(internal)?
        .iter()
        .map(crate::control::name_info)
        .collect();
    let attached: Vec<PublicKey> = edge.mesh_tunnels(&id).iter().map(|t| t.engine).collect();
    let rows = edge.store.engines(&m).await.map_err(internal)?;
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
    let rekeys = edge
        .store
        .rekey_history(&m)
        .await
        .map_err(internal)?
        .into_iter()
        .filter_map(|(old, new, ms)| {
            Some(RekeyEntry {
                old: old.parse().ok()?,
                new: new.parse().ok()?,
                ms: u64::try_from(ms).ok()?,
            })
        })
        .collect();
    let banned = detail.banned.as_deref().and_then(|b| {
        let v: Value = serde_json::from_str(b).ok()?;
        Some(BanInfo {
            reason: v.get("reason")?.as_str()?.to_string(),
            ms: v.get("ms").and_then(Value::as_u64).unwrap_or(0),
            by: v.get("by").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
    });
    typed(&MeshResult {
        mesh: id,
        invited: row.invited != 0,
        net: detail.net,
        asn: detail.asn.and_then(|a| u32::try_from(a).ok()),
        created_ms: u64::try_from(detail.created_ms).unwrap_or(0),
        names,
        engines,
        revoked,
        rekeys,
        rekeyed_to: row.rekeyed_to.and_then(|t| t.parse().ok()),
        stop_all_ms: row.stop_all_ms.and_then(|v| u64::try_from(v).ok()),
        banned,
    })
}

/// `release <name>`: as the holder's `DELETE`, with the cooldown for anyone but the previous owner.
async fn release(edge: &Arc<Edge>, name: &str, now: u64) -> AdminResult {
    let domain = edge.cfg.domain.as_str();
    let Some(row) = edge.store.name(domain, name).await.map_err(internal)? else {
        return Err((ErrorCode::NotFound, format!("no name {name}")));
    };
    let owner = row.owner.clone().filter(|_| row.held()).ok_or_else(|| {
        (ErrorCode::NotFound, format!("{name} is not held"))
    })?;
    let cooldown = u64::try_from(edge.cfg.reclaim_cooldown.as_millis()).unwrap_or(u64::MAX);
    let Some(reclaim_after_ms) = edge
        .store
        .release(domain, name, &owner, now, cooldown)
        .await
        .map_err(internal)?
    else {
        return Err((ErrorCode::NotFound, format!("{name} is not held")));
    };
    let closed = edge.drop_routes(|_, r| r.name == name).len();
    crate::k8s::gone(edge, name);
    let (e, n) = (edge.clone(), name.to_string());
    tokio::spawn(async move {
        crate::certs::retire(&e, &n, crate::acme::Reason::CessationOfOperation).await;
    });
    typed(&ReleaseResult {
        name: name.to_string(),
        reclaim_after_ms,
        closed_routes: closed as u64,
    })
}

/// `recover <name|mesh> --to-mesh <id>` (§5.7, operator judgement; audited with `--ref` by [`handle`]). A mesh
/// target moves every name it holds and closes its tunnels (`kill {revoked}`); a name target moves that name and
/// drops its routes.
async fn recover(edge: &Arc<Edge>, target: &str, to: PublicKey, now: u64) -> AdminResult {
    let domain = edge.cfg.domain.as_str();
    let to_s = to.to_string();
    let whole_mesh = target.parse::<PublicKey>().ok();
    let (from, names) = match whole_mesh {
        Some(m) if edge.store.mesh(&m.to_string()).await.map_err(internal)?.is_some() => (m, Vec::new()),
        _ => {
            let row = edge.store.name(domain, target).await.map_err(internal)?;
            let Some(owner) = row.filter(|r| r.held()).and_then(|r| r.owner) else {
                return Err((ErrorCode::NotFound, format!("no held name or known mesh {target}")));
            };
            let from: PublicKey = owner.parse().map_err(|_| internal(anyhow::anyhow!("bad owner {owner}")))?;
            (from, vec![target.to_string()])
        }
    };
    if from == to {
        return Err((ErrorCode::Invalid, "to_mesh already holds it".into()));
    }
    if edge.store.ban_reason("mesh", &to_s).await.map_err(internal)?.is_some() {
        return Err((ErrorCode::Banned, format!("mesh {to_s} is banned")));
    }
    let moved = edge
        .store
        .recover(domain, &from.to_string(), &to_s, &names, now)
        .await
        .map_err(internal)?;
    edge.drop_routes(|_, r| r.mesh == from && moved.contains(&r.name));
    let mut closed = 0u64;
    if names.is_empty() {
        for t in edge.mesh_tunnels(&from) {
            edge.detach(&t);
            t.kill(None, KillReason::Revoked);
            closed += 1;
        }
    }
    tracing::warn!(%from, %to, ?moved, "admin recover");
    typed(&RecoverResult {
        from_mesh: from,
        to_mesh: to,
        names: moved,
        closed_tunnels: closed,
    })
}

/// `caa-check` and the daily re-check (§8.2): resolve `D`'s CAA set against the primary CA's lines; with
/// `require_caa` a failure flips admission to `closed`.
pub async fn caa_check(edge: &Edge, now: u64) -> anyhow::Result<CaaCheckResult> {
    let Some(certs) = edge.certs.get() else {
        anyhow::bail!("no certificate backends on this edge");
    };
    let records = certs.caa.caa(&edge.cfg.domain).await?;
    let ca = certs.ca.id();
    let method = crate::acme::validation_method(&edge.cfg, ca);
    let ok = crate::acme::caa_ok(&records, crate::acme::issuer_domain(ca), certs.ca.account_uri(), &method);
    let missing = if ok {
        Vec::new()
    } else {
        crate::acme::caa_lines(&edge.cfg.domain, ca, certs.ca.account_uri(), &method).to_vec()
    };
    if !ok && edge.cfg.require_caa {
        tracing::error!(domain = %edge.cfg.domain, "CAA check failed: admission closed");
        *edge.admission.write().expect("admission lock") = Admission::Closed;
    }
    Ok(CaaCheckResult {
        ok,
        found: records.iter().map(|r| format!("{} \"{}\"", r.tag, r.value)).collect(),
        missing,
        admission: edge.admission(),
        checked_ms: now,
    })
}

async fn revoke_cert(edge: &Arc<Edge>, name: &str, reason: RevokeReason, now: u64) -> AdminResult {
    let domain = edge.cfg.domain.as_str();
    let Some(certs) = edge.certs.get() else {
        return Err((ErrorCode::Invalid, "no certificate backends on this edge".into()));
    };
    // `revocations.reason` has no `unspecified`: it goes out as `cessationOfOperation`.
    let (r, sent) = match reason {
        RevokeReason::KeyCompromise => (crate::acme::Reason::KeyCompromise, RevokeReason::KeyCompromise),
        RevokeReason::Superseded => (crate::acme::Reason::Superseded, RevokeReason::Superseded),
        RevokeReason::CessationOfOperation | RevokeReason::Unspecified => {
            (crate::acme::Reason::CessationOfOperation, RevokeReason::CessationOfOperation)
        }
    };
    if !edge.store.revoke_name_chain(domain, name, r.as_str(), now).await.map_err(internal)? {
        return Err((ErrorCode::NotFound, format!("{name} has no certificate")));
    }
    crate::certs::revoke_due(edge, certs, now).await.map_err(internal)?;
    typed(&RevokeCertResult { name: name.to_string(), reason: sent })
}

/// `cert retry <name>`: the name's queued job due now, attempts reset, runner woken.
async fn cert_retry(edge: &Arc<Edge>, name: &str, now: u64) -> AdminResult {
    let Some(certs) = edge.certs.get() else {
        return Err((ErrorCode::Invalid, "no certificate backends on this edge".into()));
    };
    let Some(job) = edge.store.open_job(&edge.cfg.domain, name).await.map_err(internal)? else {
        return Err((ErrorCode::NotFound, format!("{name} has no queued certificate job")));
    };
    if job.state != "queued" || !edge.store.retry_job(&job.id, now).await.map_err(internal)? {
        return Err((ErrorCode::Invalid, format!("{name}: its job is being ordered now")));
    }
    certs.wake.notify_one();
    typed(&CertRetryResult {
        name: name.to_string(),
        job_id: job.id,
        kind: match job.kind.as_str() {
            "renew" => JobKind::Renew,
            "rotate" => JobKind::Rotate,
            _ => JobKind::New,
        },
        attempts_before: u32::try_from(job.attempts).unwrap_or(0),
        last_error: job.last_error,
        next_at_ms: now,
    })
}

/// `reload` (§8.1): apply the reloadable keys of `cfg` live and name the changed ones; keys sized at start are
/// reported in `restart_required` and not applied.
pub fn reload(edge: &Edge, cfg: &crate::config::Config) -> anyhow::Result<ReloadResult> {
    let old = edge.live();
    let new = Live::from_config(cfg)?;
    let mut changed = Vec::new();
    let mut diff = |key: &str, a: String, b: String| {
        if a != b {
            changed.push(key.to_string());
        }
    };
    diff("limits", format!("{:?}", old.limits), format!("{:?}", new.limits));
    diff("brand_blocklist_file", format!("{:?}", old.brands), format!("{:?}", new.brands));
    diff("reserved_words", format!("{:?}", old.reserved_words), format!("{:?}", new.reserved_words));
    diff("terms_url", old.terms_url.clone(), new.terms_url.clone());
    diff("terms_rev", old.terms_rev.clone(), new.terms_rev.clone());
    diff("abuse_url", old.abuse_url.clone(), new.abuse_url.clone());
    diff("acme.queue_max", old.queue_max.to_string(), new.queue_max.to_string());
    diff("acme.invite_share", old.invite_share.to_string(), new.invite_share.to_string());
    diff("brake", format!("{:?}", old.brake), format!("{:?}", new.brake));
    diff("admission", format!("{:?}", old.admission), format!("{:?}", new.admission));
    let mut restart_required = Vec::new();
    let mut fixed = |key: &str, a: String, b: String| {
        if a != b {
            restart_required.push(key.to_string());
        }
    };
    let (s, n) = (&edge.cfg, cfg);
    fixed("domain", s.domain.clone(), n.domain.clone());
    fixed("data_dir", format!("{:?}", s.data_dir), format!("{:?}", n.data_dir));
    fixed("listen", s.listen.to_string(), n.listen.to_string());
    fixed("iroh", format!("{:?}", s.iroh), format!("{:?}", n.iroh));
    fixed("admin", format!("{:?}", s.admin), format!("{:?}", n.admin));
    fixed("dns", format!("{:?}", s.dns), format!("{:?}", n.dns));
    fixed("k8s", format!("{:?}", s.k8s), format!("{:?}", n.k8s));
    fixed("acme.cas", format!("{:?}", s.acme.cas), format!("{:?}", n.acme.cas));
    let (sl, nl) = (&s.limits, &n.limits);
    fixed("limits.global_conns", sl.global_conns.to_string(), nl.global_conns.to_string());
    fixed("limits.pre_hello_conns", sl.pre_hello_conns.to_string(), nl.pre_hello_conns.to_string());
    fixed("limits.new_conns_per_ip_sec", sl.new_conns_per_ip_sec.to_string(), nl.new_conns_per_ip_sec.to_string());
    fixed("limits.info_per_ip_min", sl.info_per_ip_min.to_string(), nl.info_per_ip_min.to_string());
    fixed("limits.asn_db", format!("{:?}", sl.asn_db), format!("{:?}", nl.asn_db));
    if new.admission != old.admission {
        *edge.admission.write().expect("admission lock") = new.admission;
    }
    edge.set_live(new);
    tracing::info!(?changed, ?restart_required, "config reloaded");
    Ok(ReloadResult { changed, restart_required })
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
            // §6.3: a ban revokes (`cessationOfOperation`; `keyCompromise` on a report is TODO(210-M4)) and
            // deletes the name's records.
            let (e, n) = (edge.clone(), value.to_string());
            tokio::spawn(async move {
                crate::certs::retire(&e, &n, crate::acme::Reason::CessationOfOperation).await;
            });
            let removed = edge.drop_routes(|_, r| r.name == value);
            crate::k8s::gone(edge, value);
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
