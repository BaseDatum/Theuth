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
use share_edge::store::{NewName, Store};
use share_proto::{ErrorCode, PublicKey, SecretKey};

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
        .claim(NewName {
            domain: "tun.zip",
            name,
            kind: "reserved",
            owner: &m,
            now_ms: now_ms() as i64,
            net: "192.0.2.1",
        })
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
    let first = certs::submit(&r.edge, &mesh, name, &der, false).await.unwrap().unwrap();
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
    match certs::submit(&r.edge, &mesh, "sky", &der, false).await.unwrap().unwrap() {
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
    let e = certs::submit(&r.edge, &mesh, "sky", &csr("evil"), false).await.unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::CsrNames);
    let e = certs::submit(&r.edge, &mesh, "sky", &csr_for(&["sky.tun.zip"]), false)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::CsrNames);
    let other = SecretKey::generate().public();
    let e = certs::submit(&r.edge, &other, "sky", &csr("sky"), false).await.unwrap().unwrap_err();
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
    let e = certs::submit(&r.edge, &mesh, "sky", &new, false).await.unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::CsrMismatch);
    assert!(e.spki.is_some());

    let p = certs::submit(&r.edge, &mesh, "sky", &new, true).await.unwrap().unwrap();
    assert!(matches!(p, Submitted::Pending(_)));
    certs::tick(&r.edge, now_ms()).await.unwrap();
    let row = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap();
    assert_ne!(row.chain_pem.as_deref(), Some(old.as_str()));
    assert_eq!(r.f.ca.revoked.lock().unwrap().clone(), vec![(old.clone(), Reason::Superseded)]);
    assert_eq!(r.f.ca.replaces.lock().unwrap()[1].as_deref(), Some(old.as_str()));
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
        .store_chain("tun.zip", "sky", "letsencrypt", row.chain_pem.as_deref().unwrap(), row.not_after_ms.unwrap() as u64, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn release_revokes_and_deletes_records() {
    let r = rig("").await;
    let (_, _) = issue(&r, "sky").await;
    let chain = r.edge.store.cert_row("tun.zip", "sky").await.unwrap().unwrap().chain_pem.unwrap();
    r.edge.store.release("tun.zip", "sky", now_ms(), now_ms() + DAY).await.unwrap();
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
    claim(&r.edge, "old").await;
    for (t, host, v, n) in [
        (Rtype::Cname, "old", "edge.tun.zip", "old"),
        (Rtype::Cname, "*.old", "edge.tun.zip", "old"),
        (Rtype::Txt, "_acme-challenge.sky", "stale", "*.sky.tun.zip"),
    ] {
        r.edge.store.dns_add(host, t.as_str(), v, n, now).await.unwrap();
        r.f.zone.put(t, host, v);
    }
    r.edge.store.set_records("tun.zip", "old", true).await.unwrap();
    r.edge.store.release("tun.zip", "old", now, now + DAY).await.unwrap();

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
    certs::submit(&r.edge, &mesh, "sky", &csr("sky"), false).await.unwrap().unwrap();
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
    let p = certs::submit(&r.edge, &mesh, "fog", &csr("fog"), false).await.unwrap().unwrap();
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
