//! M4 operations over `admin::handle` against an in-process edge with fakes (210 §8.5, §9.5, §5.5): invites,
//! queue, mesh, release, recover, evict, revoke-cert, caa-check, reload, the brake with injected time, the
//! standby key and retire, drain.

mod common;

use std::sync::Arc;

use share_edge::certs::{self, Certs};
use share_edge::config::{Config, Secrets};
use share_edge::state::{Edge, now_ms};
use share_edge::store::{NewName, Store};
use share_proto::admin::{
    AdminCommand, AdminRequest, AdminResponse, BrakeKind, BrakeStatus, CaaCheckResult, CertRetryResult,
    InviteListResult, InviteMinted, InviteRevoked, JobKind, MeshResult, QueueResult, RecoverResult, ReleaseResult,
    ReloadResult, RevokeCertResult, RevokeReason, StatusResult,
};
use share_proto::{Admission, CsrQuery, ErrorBody, ErrorCode, LimitKind, LimitScope, PublicKey, SecretKey};

struct Rig {
    edge: Arc<Edge>,
    _f: common::Fakes,
    dir: tempfile::TempDir,
}

async fn rig(extra: &str) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let yaml = format!(
        "domain: tun.zip\ndata_dir: {}\nacme: {{propagation: 0, auth_gap: 0}}\n{extra}",
        dir.path().display()
    );
    let (cfg, _) = Config::from_yaml(&yaml, &|_| None).unwrap();
    let store = Store::open(&cfg.db_path(), cfg.db_journal).await.unwrap();
    let f = common::fakes();
    let edge = Arc::new(Edge::new(cfg, Secrets::default(), store.clone(), SecretKey::generate().public()).unwrap());
    edge.certs.set(Certs::new(&f.backends, store, &edge.cfg)).unwrap();
    Rig { edge, _f: f, dir }
}

async fn ask(edge: &Arc<Edge>, command: AdminCommand) -> AdminResponse {
    share_edge::admin::handle(edge, AdminRequest { id: 7, command, reference: Some("T-1".into()) }).await
}

async fn ok<T: serde::de::DeserializeOwned>(edge: &Arc<Edge>, command: AdminCommand) -> T {
    let r = ask(edge, command).await;
    assert!(r.ok, "{r:?}");
    r.result_as::<T>().unwrap().unwrap()
}

async fn claim(edge: &Edge, mesh: &PublicKey, name: &str) {
    let m = mesh.to_string();
    edge.store.attach(&m, &SecretKey::generate().public().to_string(), "192.0.2.1", false, now_ms()).await.unwrap();
    edge.store
        .claim(
            NewName { domain: "tun.zip", name, kind: "reserved", owner: &m, now_ms: now_ms() as i64, net: "192.0.2.1" },
            3,
        )
        .await
        .unwrap();
}

fn csr(name: &str) -> Vec<u8> {
    let key = rcgen::KeyPair::generate().unwrap();
    let params =
        rcgen::CertificateParams::new(vec![format!("{name}.tun.zip"), format!("*.{name}.tun.zip")]).unwrap();
    params.serialize_request(&key).unwrap().der().to_vec()
}

#[tokio::test]
async fn invites_mint_list_revoke_and_admit_once() {
    let r = rig("").await;
    let m: InviteMinted = ok(
        &r.edge,
        AdminCommand::InviteNew { uses: Some(1), expires_s: Some(3600), note: Some("alice".into()) },
    )
    .await;
    let invite = m.ticket.invite.expect("the ticket carries the invite");
    assert_eq!(m.ticket.edge_id(), r.edge.id);
    assert_eq!(m.hash, invite.hash_hex());
    let l: InviteListResult = ok(&r.edge, AdminCommand::InviteList).await;
    assert_eq!(l.invites.len(), 1);
    assert_eq!(l.invites[0].note.as_deref(), Some("alice"));
    assert!(l.invites[0].hash == m.hash, "list shows the hash, never the secret");
    // hello consumes it once.
    assert!(r.edge.store.consume_invite(&invite.hash(), now_ms()).await.unwrap());
    assert!(!r.edge.store.consume_invite(&invite.hash(), now_ms()).await.unwrap());

    let m2: InviteMinted = ok(&r.edge, AdminCommand::InviteNew { uses: None, expires_s: None, note: None }).await;
    let short = ask(&r.edge, AdminCommand::InviteRevoke { hash: m2.hash[..4].into() }).await;
    assert_eq!(short.error.unwrap().code, ErrorCode::Invalid);
    let rv: InviteRevoked = ok(&r.edge, AdminCommand::InviteRevoke { hash: m2.hash[..10].to_uppercase() }).await;
    assert_eq!(rv.hash, m2.hash);
    let again = ask(&r.edge, AdminCommand::InviteRevoke { hash: m2.hash.clone() }).await;
    assert_eq!(again.error.unwrap().code, ErrorCode::NotFound);
    assert!(!r.edge.store.consume_invite(&m2.ticket.invite.unwrap().hash(), now_ms()).await.unwrap());
}

#[tokio::test]
async fn queue_mesh_release_recover_and_revoke_cert() {
    let r = rig("").await;
    let a = SecretKey::generate().public();
    let b = SecretKey::generate().public();
    claim(&r.edge, &a, "alpha").await;
    claim(&r.edge, &a, "beta").await;
    // One name waits in the queue: drain the bucket first.
    let first = certs::submit(&r.edge, &a, "alpha", &csr("alpha"), CsrQuery::default()).await.unwrap().unwrap();
    assert!(matches!(first, certs::Submitted::Pending(_)));
    let q: QueueResult = ok(&r.edge, AdminCommand::Queue).await;
    assert_eq!(q.jobs.len(), 1);
    assert_eq!(q.jobs[0].name, "alpha");
    assert_eq!(q.jobs[0].mesh, a);
    assert_eq!(q.jobs[0].position, 1);
    certs::tick(&r.edge, now_ms()).await.unwrap();
    let q: QueueResult = ok(&r.edge, AdminCommand::Queue).await;
    assert!(q.jobs.is_empty(), "issued: no longer waiting");

    let m: MeshResult = ok(&r.edge, AdminCommand::Mesh { id: a }).await;
    assert_eq!(m.names.len(), 2);
    assert_eq!(m.engines.len(), 2);
    assert!(m.rekeys.is_empty());
    let none = ask(&r.edge, AdminCommand::Mesh { id: b }).await;
    assert_eq!(none.error.unwrap().code, ErrorCode::NotFound);

    let rc: RevokeCertResult =
        ok(&r.edge, AdminCommand::RevokeCert { name: "alpha".into(), reason: Some(RevokeReason::KeyCompromise) })
            .await;
    assert_eq!(rc.reason, RevokeReason::KeyCompromise);
    let revs = r.edge.store.revocations("tun.zip", "alpha").await.unwrap();
    assert!(
        revs.iter().any(|v| v.reason == "key_compromise"),
        "{:?}",
        revs.iter().map(|v| (&v.reason, &v.state)).collect::<Vec<_>>()
    );
    let gone = ask(&r.edge, AdminCommand::RevokeCert { name: "alpha".into(), reason: None }).await;
    assert_eq!(gone.error.unwrap().code, ErrorCode::NotFound);

    let rec: RecoverResult =
        ok(&r.edge, AdminCommand::Recover { target: "beta".into(), to_mesh: b }).await;
    assert_eq!((rec.from_mesh, rec.to_mesh, rec.names.clone()), (a, b, vec!["beta".to_string()]));
    let rec: RecoverResult = ok(&r.edge, AdminCommand::Recover { target: a.to_string(), to_mesh: b }).await;
    assert_eq!(rec.names, vec!["alpha".to_string()]);
    let mb: MeshResult = ok(&r.edge, AdminCommand::Mesh { id: b }).await;
    assert_eq!(mb.names.len(), 2);

    let rel: ReleaseResult = ok(&r.edge, AdminCommand::Release { name: "beta".into() }).await;
    assert!(rel.reclaim_after_ms > now_ms());
    let rel2 = ask(&r.edge, AdminCommand::Release { name: "beta".into() }).await;
    assert_eq!(rel2.error.unwrap().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn caa_check_status_and_reload() {
    let r = rig("require_caa: true\n").await;
    let c: CaaCheckResult = ok(&r.edge, AdminCommand::CaaCheck).await;
    assert!(c.ok, "{c:?}");
    assert!(c.missing.is_empty());
    assert_eq!(c.admission, Admission::Open);
    let s: StatusResult = ok(&r.edge, AdminCommand::Status).await;
    assert_eq!(s.names_max, 1200);
    assert_eq!(s.queue_max, 2000);

    let not_from_file = ask(&r.edge, AdminCommand::Reload).await;
    assert_eq!(not_from_file.error.unwrap().code, ErrorCode::Invalid);
    let brands = r.dir.path().join("brands.txt");
    std::fs::write(&brands, "acmebank\n").unwrap();
    let yaml = format!(
        "domain: tun.zip\ndata_dir: {}\nadmission: invite\nterms_rev: r2\nbrand_blocklist_file: {}\n\
         limits: {{names_max: 7, global_conns: 5}}\nlisten: \"127.0.0.1:1\"\n",
        r.dir.path().display(),
        brands.display()
    );
    let (cfg, _) = Config::from_yaml(&yaml, &|_| None).unwrap();
    let rr: ReloadResult = share_edge::admin::reload(&r.edge, &cfg).unwrap();
    for k in ["limits", "brand_blocklist_file", "terms_rev", "admission"] {
        assert!(rr.changed.iter().any(|c| c == k), "{k} in {rr:?}");
    }
    assert!(rr.restart_required.iter().any(|c| c == "listen"));
    assert!(rr.restart_required.iter().any(|c| c == "limits.global_conns"));
    assert_eq!(r.edge.admission(), Admission::Invite);
    assert_eq!(r.edge.live().limits.names_max, 7);
    assert_eq!(r.edge.live().brands, vec!["acmebank".to_string()]);
    assert_eq!(r.edge.live().terms_rev, "r2");
}

#[tokio::test]
async fn the_brake_pulls_invite_on_a_refusal_spike_and_says_why() {
    let r = rig("brake: {auto: true, refusals_per_min: 3, new_meshes_per_min: 0}\n").await;
    let t0 = now_ms();
    let st = share_edge::ops::brake_tick(&r.edge, t0).await.unwrap();
    assert!(!st.pull, "{st:?}");
    assert_eq!(r.edge.admission(), Admission::Open);
    for i in 0..3 {
        r.edge
            .ops
            .refused(&ErrorBody::limited(ErrorCode::NetworkQuota, LimitScope::Net, LimitKind::Meshes), t0 + i);
    }
    // Banned is not a limit refusal.
    r.edge.ops.refused(&ErrorBody::new(ErrorCode::Banned), t0);
    let st = share_edge::ops::brake_tick(&r.edge, t0 + 10).await.unwrap();
    assert!(st.pull);
    let spike = st.conditions.iter().find(|c| c.kind == BrakeKind::AbuseSpike).unwrap();
    assert!(spike.holds);
    assert!(spike.detail.as_deref().unwrap().contains("3/min"), "{spike:?}");
    assert_eq!(r.edge.admission(), Admission::Invite, "auto brake flips open → invite");
    assert_eq!(share_edge::ops::pulled(&r.edge).unwrap().1, vec![BrakeKind::AbuseSpike]);
    // A minute later the spike has cleared; admission stays `invite` (back to open is the operator's).
    let s: BrakeStatus = ok(&r.edge, AdminCommand::BrakeStatus).await;
    assert_eq!(s.admission, Admission::Invite);
    let st = share_edge::ops::brake_status(&r.edge, t0 + 70_000).await.unwrap();
    assert!(!st.pull);
    assert_eq!(st.clear_days, Some(0));
    assert_eq!(st.admission, Admission::Invite);
    let st = share_edge::ops::brake_status(&r.edge, t0 + 3 * 86_400_000).await.unwrap();
    assert_eq!(st.clear_days, Some(2));

    let m = share_edge::ops::render(&r.edge, t0 + 10).await;
    assert!(m.contains("share_edge_refusals_total{code=\"network_quota\",scope=\"net\",limit=\"meshes\"} 3"), "{m}");
    assert!(m.contains("share_edge_queue_depth{class=\"open\"} 0"), "{m}");
    assert!(m.contains("share_edge_slots_left{ca=\"letsencrypt\",class=\"invited\"}"), "{m}");
    assert!(m.contains("share_edge_admission{mode=\"invite\"} 1"), "{m}");
    assert!(m.contains("share_edge_brake_pulled 1"), "{m}");
}

#[tokio::test]
async fn edge_full_holds_the_brake_without_auto() {
    let r = rig("").await;
    r.edge.set_full(true);
    let st = share_edge::ops::brake_tick(&r.edge, now_ms()).await.unwrap();
    assert!(st.pull);
    assert!(st.conditions.iter().any(|c| c.kind == BrakeKind::EdgeFull && c.holds));
    assert_eq!(r.edge.admission(), Admission::Open, "no auto: only reported");
}

#[tokio::test]
async fn standby_ticket_and_retire_promote_the_standby_key() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = format!("domain: tun.zip\ndata_dir: {}\niroh: {{relay: \"https://r.example/\"}}\n", dir.path().display());
    let (cfg, _) = Config::from_yaml(&yaml, &|_| None).unwrap();
    let live = share_edge::load_or_create_key(&cfg.key_path()).unwrap();
    let standby = share_edge::standby_ticket(&cfg).unwrap();
    assert_ne!(standby.edge_id(), live.public());
    assert_eq!(share_edge::standby_ticket(&cfg).unwrap().edge_id(), standby.edge_id(), "stored, stable");
    let signed = share_edge::retire(&cfg, 1234).unwrap();
    let mut pins = share_proto::EdgePins::new(live.public(), Some(standby.edge_id()));
    let notice = pins.accept_retire(&signed, &standby.edge_id()).unwrap();
    assert_eq!(notice.retired, live.public());
    assert_eq!(share_edge::load_or_create_key(&cfg.key_path()).unwrap().public(), standby.edge_id());
    assert!(cfg.retire_path().exists());
    assert!(share_edge::retire(&cfg, 1235).is_err(), "no standby left to promote");
}

#[tokio::test]
async fn drain_refuses_hello_and_returns_at_once_without_tunnels() {
    let r = rig("").await;
    assert_eq!(share_edge::drain(&r.edge, 30).await, 0);
    assert!(r.edge.draining.load(std::sync::atomic::Ordering::Relaxed));
}

#[tokio::test]
async fn cert_retry_makes_a_backed_off_job_due_and_resets_attempts() {
    let r = rig("").await;
    let a = SecretKey::generate().public();
    claim(&r.edge, &a, "alpha").await;
    certs::submit(&r.edge, &a, "alpha", &csr("alpha"), CsrQuery::default()).await.unwrap().unwrap();
    // Two failed attempts; the job now waits far out in backoff.
    let t0 = now_ms();
    let far = t0 + 3_600_000;
    for _ in 0..2 {
        let j = r.edge.store.lease_job(far, far + 60_000).await.unwrap().unwrap();
        r.edge.store.requeue_job(&j.id, far, Some("dns: SERVFAIL"), false).await.unwrap();
    }
    let before = r.edge.store.open_job("tun.zip", "alpha").await.unwrap().unwrap();
    assert_eq!(before.attempts, 2);

    let res: CertRetryResult = ok(&r.edge, AdminCommand::CertRetry { name: "alpha".into() }).await;
    assert_eq!(res.name, "alpha");
    assert_eq!(res.job_id, before.id);
    assert_eq!(res.kind, JobKind::New);
    assert_eq!(res.attempts_before, 2);
    assert_eq!(res.last_error.as_deref(), Some("dns: SERVFAIL"));
    assert!(res.next_at_ms >= t0 && res.next_at_ms < far);
    let after = r.edge.store.open_job("tun.zip", "alpha").await.unwrap().unwrap();
    assert_eq!((after.state.as_str(), after.attempts, after.next_at_ms as u64), ("queued", 0, res.next_at_ms));
    // Due now: a runner tick as of `now` leases it.
    let leased = r.edge.store.lease_job(now_ms(), now_ms() + 60_000).await.unwrap().unwrap();
    assert_eq!((leased.id.as_str(), leased.attempts), (before.id.as_str(), 1));

    // While ordering: refused, untouched. Unknown or job-less name: not_found.
    let busy = ask(&r.edge, AdminCommand::CertRetry { name: "alpha".into() }).await;
    assert_eq!(busy.error.unwrap().code, ErrorCode::Invalid);
    let none = ask(&r.edge, AdminCommand::CertRetry { name: "nobody".into() }).await;
    assert_eq!(none.error.unwrap().code, ErrorCode::NotFound);
}
