//! The certificate path against a fake zone and a fake CA (210 §6): issuance, renewal, revocation, orphan
//! reconciliation, the CSR rule, panic containment and the new-certificate bucket.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use share_edge::acme::{Reason, Window, leaf_info};
use share_edge::certs::{self, Certs, Submitted};
use share_edge::config::{Config, Secrets};
use share_edge::dns::Rtype;
use share_edge::state::{Edge, now_ms};
use share_edge::store::{Claimed, NewName, Store};
use share_proto::{CsrQuery, ErrorCode, PublicKey, SecretKey};

const PLAIN: CsrQuery = CsrQuery { rotate: false, compromised: false };
const ROTATE: CsrQuery = CsrQuery { rotate: true, compromised: false };
const ROTATE_COMPROMISED: CsrQuery = CsrQuery { rotate: true, compromised: true };

const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

struct Rig {
    edge: Arc<Edge>,
    f: common::Fakes,
    _dir: tempfile::TempDir,
}

async fn rig(extra: &str) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let yaml = format!(
        "domain: tun.zip\ndata_dir: {}\nacme: {{propagation: 0, auth_gap: 0{extra}}}\n",
        dir.path().display()
    );
    let (cfg, _) = Config::from_yaml(&yaml, &|_| None).unwrap();
    let store = Store::open(&cfg.db_path()).await.unwrap();
    let f = common::fakes();
    let edge = Arc::new(Edge::new(cfg, Secrets::default(), store.clone(), SecretKey::generate().public()).unwrap());
    edge.certs.set(Certs::new(&f.backends, store, &edge.cfg)).unwrap();
    Rig { edge, f, _dir: dir }
}

async fn claim(edge: &Edge, name: &str) -> PublicKey {
    let mesh = SecretKey::generate().public();
    let engine = SecretKey::generate().public();
    let m = mesh.to_string();
    edge.store.attach(&m, &engine.to_string(), "192.0.2.1", false, now_ms()).await.unwrap();
    edge.store
        .claim(
            NewName {
                domain: "tun.zip",
                name,
                kind: "reserved",
                owner: &m,
                now_ms: now_ms() as i64,
                net: "192.0.2.1",
            },
            3,
        )
        .await
        .unwrap();
    mesh
}

fn csr_for(sans: &[&str]) -> Vec<u8> {
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
    params.serialize_request(&key).unwrap().der().to_vec()
}

fn csr(name: &str) -> Vec<u8> {
    csr_for(&[&format!("{name}.tun.zip"), &format!("*.{name}.tun.zip")])
}

async fn issue(r: &Rig, name: &str) -> (PublicKey, Vec<u8>) {
    let mesh = claim(&r.edge, name).await;
    let der = csr(name);
    let first = certs::submit(&r.edge, &mesh, name, &der, PLAIN).await.unwrap().unwrap();
    assert!(matches!(first, Submitted::Pending(_)), "{first:?}");
    certs::tick(&r.edge, now_ms()).await.unwrap();
    (mesh, der)
}

#[tokio::test]
async fn issuance_writes_records_serializes_txt_and_serves_the_chain() {
    let r = rig("").await;
    let (mesh, der) = issue(&r, "sky").await;

    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    assert_eq!(row.state, "valid");
    assert_eq!(row.records, 1);
    let chain = row.chain_pem.unwrap();
    let leaf = leaf_info(&chain).unwrap();
    assert_eq!(leaf.spki, share_proto::check_csr(&der, "sky", "tun.zip").unwrap().spki);
    assert_eq!(row.not_after_ms.unwrap() as u64, leaf.not_after_ms);

    // D17: both CNAMEs, no challenge TXT left, never two TXTs at once.
    assert_eq!(r.f.zone.value(Rtype::Cname, "sky"), Some(vec!["edge.tun.zip".into()]));
    assert_eq!(r.f.zone.value(Rtype::Cname, "*.sky"), Some(vec!["edge.tun.zip".into()]));
    assert_eq!(r.f.zone.value(Rtype::Txt, "_acme-challenge.sky"), None);
    assert_eq!(r.f.zone.max_txt.load(Ordering::SeqCst), 1);
    let ledger = r.edge.store.dns_all().await.unwrap();
    assert_eq!(ledger.len(), 2, "{ledger:?}");

    // The same CSR again: the chain; GET cert: the chain.
    match certs::submit(&r.edge, &mesh, "sky", &der, PLAIN).await.unwrap().unwrap() {
        Submitted::Chain(c) => assert_eq!(c.chain_pem, chain),
        other => panic!("{other:?}"),
    }
    assert_eq!(certs::chain(&r.edge, &mesh, "sky").await.unwrap().unwrap().chain_pem, chain);
    assert_eq!(r.f.ca.issued.load(Ordering::SeqCst), 1);
    assert_eq!(r.f.ca.replaces.lock().unwrap()[0], None);
}

#[tokio::test]
async fn foreign_names_and_foreign_meshes_are_refused() {
    let r = rig("").await;
    let mesh = claim(&r.edge, "sky").await;
    let e = certs::submit(&r.edge, &mesh, "sky", &csr("evil"), PLAIN).await.unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::CsrNames);
    let e = certs::submit(&r.edge, &mesh, "sky", &csr_for(&["sky.tun.zip"]), PLAIN)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::CsrNames);
    let other = SecretKey::generate().public();
    let e = certs::submit(&r.edge, &other, "sky", &csr("sky"), PLAIN).await.unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::NotOwner);
    assert!(r.edge.store.jobs("tun.zip", "sky").await.unwrap().is_empty());
    assert!(r.edge.certs.get().is_some());
    assert!(r.f.zone.hosts().is_empty());
}

#[tokio::test]
async fn a_different_csr_needs_rotate_and_rotation_revokes_the_old_chain() {
    let r = rig("").await;
    let (mesh, _) = issue(&r, "sky").await;
    let old = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();
    let new = csr("sky");
    let e = certs::submit(&r.edge, &mesh, "sky", &new, PLAIN).await.unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::CsrMismatch);
    assert!(e.spki.is_some());

    let p = certs::submit(&r.edge, &mesh, "sky", &new, ROTATE).await.unwrap().unwrap();
    assert!(matches!(p, Submitted::Pending(_)));
    certs::tick(&r.edge, now_ms()).await.unwrap();
    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    assert_ne!(row.chain_pem.as_deref(), Some(old.as_str()));
    assert_eq!(r.f.ca.revoked.lock().unwrap().clone(), vec![(old.clone(), Reason::Superseded)]);
    assert_eq!(r.f.ca.replaces.lock().unwrap()[1].as_deref(), Some(old.as_str()));
}

/// §6.3 after a removal: `?rotate=true&compromised=true` revokes the old certificate `keyCompromise`, and the
/// flag does not outlive that rotation (the next plain rotation is `superseded`).
#[tokio::test]
async fn a_compromised_rotation_revokes_key_compromise_once() {
    let r = rig("").await;
    let (mesh, _) = issue(&r, "sky").await;
    let first = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();
    let p = certs::submit(&r.edge, &mesh, "sky", &csr("sky"), ROTATE_COMPROMISED).await.unwrap().unwrap();
    assert!(matches!(p, Submitted::Pending(_)));
    certs::tick(&r.edge, now_ms()).await.unwrap();
    let second = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();
    assert_ne!(first, second);
    assert!(!r.edge.store.rotate_compromised("tun.zip", "sky").await.unwrap());

    certs::submit(&r.edge, &mesh, "sky", &csr("sky"), ROTATE).await.unwrap().unwrap();
    certs::tick(&r.edge, now_ms()).await.unwrap();
    assert_eq!(
        r.f.ca.revoked.lock().unwrap().clone(),
        vec![(first, Reason::KeyCompromise), (second, Reason::Superseded)]
    );
}

/// §5.2 D20 at the store and the CA: an offered name moves to the receiving mesh without a cooldown, the old
/// chain is revoked `cessationOfOperation`, the name stays slotted (no new-name token: the bucket is empty) and
/// the receiver's own CSR is issued. Without an offer, or after it lapses, the claim is `Taken`.
#[tokio::test]
async fn an_online_transfer_moves_the_name_and_revokes_the_old_chain() {
    let r = rig(", cas: {letsencrypt: {new_certs_per_week: 1}}").await;
    let (a, _) = issue(&r, "sky").await;
    let old = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();
    let b = new_mesh(&r.edge).await;
    assert_eq!(claim_as(r.edge.clone(), b, "sky", "reserved").await, Claimed::Taken);

    // An offer that already lapsed does nothing.
    let (sa, sb) = (a.to_string(), b.to_string());
    assert!(r.edge.store.offer_transfer("tun.zip", "sky", &sa, &sb, now_ms() - 1).await.unwrap());
    assert_eq!(claim_as(r.edge.clone(), b, "sky", "reserved").await, Claimed::Taken);
    // Only the holder can offer.
    assert!(!r.edge.store.offer_transfer("tun.zip", "sky", &sb, &sa, now_ms() + DAY).await.unwrap());

    assert!(r.edge.store.offer_transfer("tun.zip", "sky", &sa, &sb, now_ms() + DAY).await.unwrap());
    let row = r.edge.store.name("tun.zip", "sky").await.unwrap().unwrap();
    assert_eq!(row.transfer_to.as_deref(), Some(sb.as_str()));
    assert_eq!(claim_as(r.edge.clone(), b, "sky", "reserved").await, Claimed::Transferred);
    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    assert_eq!(row.owner.as_deref(), Some(sb.as_str()));
    assert_eq!(row.state, "slotted");
    assert!(row.chain_pem.is_none() && row.csr.is_none());
    assert_eq!(claim_as(r.edge.clone(), a, "sky", "reserved").await, Claimed::Taken);

    let der = csr("sky");
    certs::submit(&r.edge, &b, "sky", &der, PLAIN).await.unwrap().unwrap();
    certs::tick(&r.edge, now_ms()).await.unwrap();
    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    assert_eq!(row.state, "valid");
    let leaf = leaf_info(row.chain_pem.as_deref().unwrap()).unwrap();
    assert_eq!(leaf.spki, share_proto::check_csr(&der, "sky", "tun.zip").unwrap().spki);
    assert_eq!(r.f.ca.revoked.lock().unwrap().clone(), vec![(old, Reason::CessationOfOperation)]);
    assert_eq!(r.f.ca.issued.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn renewal_follows_ari_then_falls_back_to_two_thirds() {
    // One new-name token a week: renewals must not need one.
    let r = rig(", cas: {letsencrypt: {new_certs_per_week: 1}}").await;
    let (_, _) = issue(&r, "sky").await;
    let first = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();

    // Next scan (6 h later), no ARI window and not yet at 2/3: nothing.
    let t1 = now_ms() + 7 * HOUR;
    certs::tick(&r.edge, t1).await.unwrap();
    assert_eq!(r.f.ca.issued.load(Ordering::SeqCst), 1);

    // ARI opens a window: renewal with `replaces`.
    *r.f.ca.ari.lock().unwrap() = Some(Window { start_ms: t1, end_ms: t1 + DAY });
    let t2 = t1 + 7 * HOUR;
    certs::tick(&r.edge, t2).await.unwrap();
    assert_eq!(r.f.ca.issued.load(Ordering::SeqCst), 2);
    assert_eq!(r.f.ca.replaces.lock().unwrap()[1].as_deref(), Some(first.as_str()));
    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    assert_eq!(row.state, "valid");
    assert_eq!(row.ari_start_ms, Some(t1 as i64));
    assert!(r.f.ca.revoked.lock().unwrap().is_empty(), "a renewal revokes nothing");

    // No ARI: renew at two thirds of 90 days (day 60), not before.
    *r.f.ca.ari.lock().unwrap() = None;
    r.edge.store.attach(row.owner.as_deref().unwrap(), "e", "n", false, now_ms() + 59 * DAY).await.unwrap();
    // The stored window is in the past; a CA that stops answering ARI keeps it, so clear it for this case.
    sqlx_clear_ari(&r).await;
    certs::tick(&r.edge, now_ms() + 50 * DAY).await.unwrap();
    assert_eq!(r.f.ca.issued.load(Ordering::SeqCst), 2);
    certs::tick(&r.edge, now_ms() + 61 * DAY).await.unwrap();
    assert_eq!(r.f.ca.issued.load(Ordering::SeqCst), 3);
}

async fn sqlx_clear_ari(r: &Rig) {
    // `clear_chain` would drop the chain too; re-store it without a window instead.
    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    r.edge
        .store
        .store_chain(
            "tun.zip",
            "sky",
            row.owner.as_deref().unwrap(),
            row.csr_spki.as_deref().unwrap(),
            "letsencrypt",
            row.chain_pem.as_deref().unwrap(),
            row.not_after_ms.unwrap() as u64,
            None,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn renewal_runs_while_the_new_cert_bucket_is_empty() {
    // One token a week: `sky` takes it, `fog` waits for the next one. No `hello` after the claims: the claim
    // itself counts as the last attach.
    let r = rig(", cas: {letsencrypt: {new_certs_per_week: 1}}").await;
    let (_, _) = issue(&r, "sky").await;
    let first = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();
    let mesh = claim(&r.edge, "fog").await;
    let p = certs::submit(&r.edge, &mesh, "fog", &csr("fog"), PLAIN).await.unwrap().unwrap();
    assert!(matches!(&p, Submitted::Pending(p) if p.queue_pos == Some(1)), "{p:?}");
    let t0 = now_ms() + 7 * HOUR;
    certs::tick(&r.edge, t0).await.unwrap();
    assert_eq!(r.edge.store.cert_row("tun.zip", "fog").await.unwrap().unwrap().state, "queued");

    *r.f.ca.ari.lock().unwrap() = Some(Window { start_ms: t0, end_ms: t0 + DAY });
    certs::tick(&r.edge, t0 + 7 * HOUR).await.unwrap();
    assert_eq!(r.f.ca.issued.load(Ordering::SeqCst), 2, "the renewal ran without a token");
    assert_eq!(r.f.ca.replaces.lock().unwrap()[1].as_deref(), Some(first.as_str()));
    assert_eq!(r.edge.store.cert_row("tun.zip", "fog").await.unwrap().unwrap().state, "queued", "fog still waits");
    let job = r.edge.store.open_job("tun.zip", "fog").await.unwrap().unwrap();
    assert!(job.next_at_ms as u64 > t0 + 6 * DAY, "the renewal did not touch the new-name queue");
}

#[tokio::test]
async fn release_revokes_and_deletes_records() {
    let r = rig("").await;
    let (mesh, _) = issue(&r, "sky").await;
    let chain = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();
    let m = mesh.to_string();
    r.edge.store.release("tun.zip", "sky", &m, now_ms(), DAY).await.unwrap().unwrap();
    certs::retire(&r.edge, "sky", Reason::CessationOfOperation).await;
    assert_eq!(r.f.ca.revoked.lock().unwrap().clone(), vec![(chain, Reason::CessationOfOperation)]);
    assert!(r.f.zone.hosts().is_empty(), "{:?}", r.f.zone.hosts());
    assert!(r.edge.store.dns_all().await.unwrap().is_empty());
    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    assert_eq!((row.records, row.chain_pem), (0, None));
}

#[tokio::test]
async fn reconciliation_removes_orphans_and_keeps_live_records() {
    let r = rig("").await;
    let (_, _) = issue(&r, "sky").await;
    let now = now_ms();
    // A released name whose records a crash left, and a stale challenge TXT.
    let old = claim(&r.edge, "old").await.to_string();
    for (t, host, v, n) in [
        (Rtype::Cname, "old", "edge.tun.zip", "old"),
        (Rtype::Cname, "*.old", "edge.tun.zip", "old"),
        (Rtype::Txt, "_acme-challenge.sky", "stale", "*.sky.tun.zip"),
    ] {
        r.edge.store.dns_add(host, t.as_str(), v, n, now).await.unwrap();
        r.f.zone.put(t, host, v);
    }
    r.edge.store.set_records("tun.zip", "old", true).await.unwrap();
    r.edge.store.release("tun.zip", "old", &old, now, DAY).await.unwrap().unwrap();

    certs::reconcile(&r.edge).await.unwrap();
    let mut hosts = r.f.zone.hosts();
    hosts.sort();
    assert_eq!(hosts, vec![(Rtype::Cname, "*.sky".to_string()), (Rtype::Cname, "sky".to_string())]);
    assert_eq!(r.edge.store.dns_all().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_provider_panic_is_a_failed_attempt_with_backoff() {
    let r = rig("").await;
    *r.f.zone.panic_on.lock().unwrap() = Some("*.sky".into());
    let mesh = claim(&r.edge, "sky").await;
    certs::submit(&r.edge, &mesh, "sky", &csr("sky"), PLAIN).await.unwrap().unwrap();
    let t0 = now_ms();
    certs::tick(&r.edge, t0).await.unwrap();
    let jobs = r.edge.store.jobs("tun.zip", "sky").await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!((jobs[0].state.as_str(), jobs[0].attempts), ("queued", 1));
    assert!(jobs[0].last_error.as_deref().unwrap().contains("panicked"), "{jobs:?}");
    assert!(jobs[0].next_at_ms as u64 >= t0 + 30_000, "backed off");

    // Not due yet; then due, with the provider fixed.
    *r.f.zone.panic_on.lock().unwrap() = None;
    certs::tick(&r.edge, t0 + 1_000).await.unwrap();
    assert_eq!(r.f.ca.issued.load(Ordering::SeqCst), 0);
    certs::tick(&r.edge, t0 + 2 * DAY).await.unwrap();
    assert_eq!(r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().state, "valid");
}

#[tokio::test]
async fn new_names_wait_for_a_token() {
    let r = rig(", cas: {letsencrypt: {new_certs_per_week: 1}}").await;
    let (_, _) = issue(&r, "sky").await;
    let mesh = claim(&r.edge, "fog").await;
    let p = certs::submit(&r.edge, &mesh, "fog", &csr("fog"), PLAIN).await.unwrap().unwrap();
    match p {
        Submitted::Pending(p) => assert_eq!(p.queue_pos, Some(1)),
        other => panic!("{other:?}"),
    }
    let t0 = now_ms();
    certs::tick(&r.edge, t0).await.unwrap();
    let row = r.edge.store.cert_row("tun.zip", "fog").await.unwrap().unwrap();
    assert_eq!((row.state.as_str(), row.records), ("queued", 0));
    let job = r.edge.store.open_job("tun.zip", "fog").await.unwrap().unwrap();
    assert_eq!(job.attempts, 0, "waiting for a slot is not an attempt");
    assert!(job.next_at_ms as u64 > t0 + 6 * DAY);
    certs::tick(&r.edge, t0 + 7 * DAY).await.unwrap();
    assert_eq!(r.edge.store.cert_row("tun.zip", "fog").await.unwrap().unwrap().state, "valid");
}

async fn new_mesh(edge: &Edge) -> PublicKey {
    let mesh = SecretKey::generate().public();
    let engine = SecretKey::generate().public().to_string();
    edge.store.attach(&mesh.to_string(), &engine, "192.0.2.1", false, now_ms()).await.unwrap();
    mesh
}

async fn claim_as(edge: Arc<Edge>, mesh: PublicKey, name: &'static str, kind: &'static str) -> Claimed {
    let m = mesh.to_string();
    let n = NewName { domain: "tun.zip", name, kind, owner: &m, now_ms: now_ms() as i64, net: "192.0.2.1" };
    edge.store.claim(n, 1).await.unwrap()
}

fn sorted(mut v: Vec<Claimed>) -> Vec<String> {
    let mut s: Vec<String> = v.drain(..).map(|c| format!("{c:?}")).collect();
    s.sort();
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claims_have_one_winner_and_keep_the_quota() {
    let r = rig("").await;
    let (a, b) = (new_mesh(&r.edge).await, new_mesh(&r.edge).await);

    // Two meshes, one name, at once: one wins, the other sees it taken.
    let x = tokio::spawn(claim_as(r.edge.clone(), a, "sky", "reserved"));
    let y = tokio::spawn(claim_as(r.edge.clone(), b, "sky", "reserved"));
    let got = vec![x.await.unwrap(), y.await.unwrap()];
    assert_eq!(sorted(got), ["Granted", "Taken"]);

    // One mesh, two random names at once: the quota is one random name while it holds none.
    let c = new_mesh(&r.edge).await;
    let x = tokio::spawn(claim_as(r.edge.clone(), c, "fogone", "random"));
    let y = tokio::spawn(claim_as(r.edge.clone(), c, "fogtwo", "random"));
    let got = vec![x.await.unwrap(), y.await.unwrap()];
    assert_eq!(sorted(got), ["Granted", "Quota"]);
    assert_eq!(r.edge.store.held_names("tun.zip", &c.to_string()).await.unwrap().len(), 1);

    // A released name: others wait for the cooldown, the previous owner does not.
    let owner = r.edge.store.name("tun.zip", "sky").await.unwrap().unwrap().owner.unwrap();
    let (winner, loser) = if owner == a.to_string() { (a, b) } else { (b, a) };
    r.edge.store.release("tun.zip", "sky", &owner, now_ms(), DAY).await.unwrap().unwrap();
    assert!(r.edge.store.release("tun.zip", "sky", &owner, now_ms(), DAY).await.unwrap().is_none());
    assert!(matches!(claim_as(r.edge.clone(), loser, "sky", "reserved").await, Claimed::Cooldown(_)));
    assert_eq!(claim_as(r.edge.clone(), winner, "sky", "reserved").await, Claimed::Granted);
    assert_eq!(claim_as(r.edge.clone(), winner, "sky", "reserved").await, Claimed::AlreadyHeld);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chain_ordered_before_a_release_never_lands_on_the_next_owner() {
    let r = rig("").await;
    let a = claim(&r.edge, "sky").await;
    certs::submit(&r.edge, &a, "sky", &csr("sky"), PLAIN).await.unwrap().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (go_tx, go_rx) = tokio::sync::oneshot::channel();
    *r.f.ca.gate.lock().unwrap() = Some((started_tx, go_rx));
    let edge = r.edge.clone();
    let runner = tokio::spawn(async move { certs::tick(&edge, now_ms()).await });
    started_rx.await.unwrap();

    // Mid-order: `a` releases (no cooldown), `b` claims and posts its own CSR.
    r.edge.store.release("tun.zip", "sky", &a.to_string(), now_ms(), 0).await.unwrap().unwrap();
    let b = new_mesh(&r.edge).await;
    assert_eq!(claim_as(r.edge.clone(), b, "sky", "reserved").await, Claimed::Granted);
    let p = certs::submit(&r.edge, &b, "sky", &csr("sky"), PLAIN).await.unwrap().unwrap();
    assert!(matches!(p, Submitted::Pending(_)), "{p:?}");
    go_tx.send(()).unwrap();
    runner.await.unwrap().unwrap();
    certs::tick(&r.edge, now_ms()).await.unwrap();

    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    assert_eq!(row.owner.as_deref(), Some(b.to_string().as_str()));
    let chain = row.chain_pem.expect("b's own chain");
    assert_eq!(leaf_info(&chain).unwrap().spki.0.as_slice(), row.csr_spki.as_deref().unwrap(), "b's key");
    let revoked = r.f.ca.revoked.lock().unwrap().clone();
    assert_eq!(revoked.len(), 1, "a's orphan leaf is revoked");
    assert_ne!(revoked[0].0, chain);
    assert_eq!(revoked[0].1, Reason::CessationOfOperation);
    let jobs = r.edge.store.jobs("tun.zip", "sky").await.unwrap();
    let states: Vec<_> = jobs.iter().map(|j| (j.mesh == a.to_string(), j.state.as_str())).collect();
    assert_eq!(states, [(true, "dropped"), (false, "done")]);
    // m9: each challenge TXT was polled for (as an absolute name) before the CA looked.
    let probes = r.f.zone.probes.lock().unwrap().clone();
    assert!(probes.iter().all(|p| p == "_acme-challenge.sky.tun.zip."), "{probes:?}");
    assert_eq!(probes.len(), 4, "two authorizations per order, two orders");
}

#[tokio::test]
async fn a_failed_revocation_is_retried_and_the_records_still_go() {
    let r = rig("").await;
    let (mesh, _) = issue(&r, "sky").await;
    let chain = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();
    r.f.ca.fail_revokes.store(1, Ordering::SeqCst);
    r.edge.store.release("tun.zip", "sky", &mesh.to_string(), now_ms(), DAY).await.unwrap().unwrap();
    certs::retire(&r.edge, "sky", Reason::CessationOfOperation).await;
    assert!(r.f.ca.revoked.lock().unwrap().is_empty());
    assert!(r.f.zone.hosts().is_empty(), "records go even when the CA fails: {:?}", r.f.zone.hosts());
    let rev = r.edge.store.revocations("tun.zip", "sky").await.unwrap();
    assert_eq!(rev.len(), 1);
    assert_eq!((rev[0].state.as_str(), rev[0].attempts), ("queued", 1));
    assert!(rev[0].last_error.as_deref().unwrap().contains("CA unavailable"));
    assert!(r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.is_none());

    certs::tick(&r.edge, now_ms()).await.unwrap();
    assert!(r.f.ca.revoked.lock().unwrap().is_empty(), "backoff before the retry");
    certs::tick(&r.edge, now_ms() + 3 * 60_000).await.unwrap();
    assert_eq!(r.f.ca.revoked.lock().unwrap().clone(), vec![(chain, Reason::CessationOfOperation)]);
    let rev = r.edge.store.revocations("tun.zip", "sky").await.unwrap();
    assert_eq!((rev[0].state.as_str(), rev[0].attempts), ("done", 2));
}
