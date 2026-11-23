//! Name certificates on Path Z (210 §6.0–§6.5, D7, D17): the CSR intake behind `POST /v1/names/{name}/csr`,
//! the durable `cert_jobs` runner (leases, backon retries, one order per name, serialized authorizations),
//! the per-name CNAME records, ARI renewal, revocation, the new-certificate token bucket, and DNS
//! reconciliation at start.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::bail;
use backon::BackoffBuilder as _;
use futures::FutureExt as _;
use futures::future::BoxFuture;
use share_proto::events::{CertEvent, QueueEvent};
use share_proto::{
    CertChain, CertPending, CsrError, CsrQuery, ErrorBody, ErrorCode, Event, PublicKey, SpkiHash,
};
use tokio::sync::Notify;

use crate::acme::{Ca, CaaLookup, Reason, Validator, leaf_info};
use crate::dns::{self, DnsZone, Rtype};
use crate::state::{Edge, now_ms};
use crate::store::{CertRow, JobRow, Store};

/// A name unattached this long is not renewed (§6.3).
pub const NO_RENEW_AFTER_MS: u64 = 90 * 86_400_000;
/// ETA per job ahead (§6.5: 3 min per order).
const ETA_PER_JOB_S: u64 = 180;

/// What the edge needs to issue: one zone, one CA, a CAA resolver.
#[derive(Clone, Debug)]
pub struct Backends {
    /// The DNS zone of `D`.
    pub zone: Arc<dyn DnsZone>,
    /// The primary CA.
    pub ca: Arc<dyn Ca>,
    /// CAA resolution for `require_caa`.
    pub caa: Arc<dyn CaaLookup>,
    /// Challenge TXT visibility on the zone's authoritative servers.
    pub probe: Arc<dyn dns::TxtProbe>,
}

/// The certificate subsystem, held by `Edge::certs`.
#[derive(Debug)]
pub struct Certs {
    /// The DNS zone.
    pub zone: Arc<dyn DnsZone>,
    /// The CA.
    pub ca: Arc<dyn Ca>,
    /// CAA resolution (`admin caa-check`, the daily re-check).
    pub caa: Arc<dyn CaaLookup>,
    /// The `dns-01` validator over `zone`.
    pub validator: Dns01,
    /// Wakes the runner (a new job).
    pub wake: Notify,
    last_scan_ms: Mutex<u64>,
    /// Last `queue` event position per waiting job (§6.5 events on change only).
    positions: Mutex<HashMap<String, u32>>,
    revoking: tokio::sync::Mutex<()>,
    /// One lock per name over its D17 records (FIX5-EDGE R5): `ensure_records`, `retire` and `reconcile_dns`
    /// each hold it from their ownership check through their DNS writes, so a retire cannot delete between an
    /// ensure's ledger write and its zone write (nor an ensure write after a retire's delete).
    record_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Certs {
    /// Build from the backends and the edge's config.
    pub fn new(b: &Backends, store: Store, cfg: &crate::config::Config) -> Certs {
        Certs {
            zone: b.zone.clone(),
            ca: b.ca.clone(),
            caa: b.caa.clone(),
            validator: Dns01 {
                zone: b.zone.clone(),
                store,
                dns_zone: cfg.dns_zone().to_string(),
                propagation: cfg.acme.propagation,
                probe: b.probe.clone(),
                gap: cfg.acme.auth_gap,
                last_cleanup: Mutex::new(None),
            },
            wake: Notify::new(),
            last_scan_ms: Mutex::new(0),
            positions: Mutex::new(HashMap::new()),
            revoking: tokio::sync::Mutex::new(()),
            record_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Hold `name`'s record lock. Entries nobody holds or waits for are dropped on the next call.
    async fn lock_name(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let m = {
            let mut locks = self.record_locks.lock().expect("record locks");
            locks.retain(|_, m| Arc::strong_count(m) > 1);
            locks.entry(name.to_string()).or_default().clone()
        };
        m.lock_owned().await
    }
}

/// `dns-01` over the zone: one TXT at `_acme-challenge.<n>` at a time, then a poll of the zone's authoritative
/// servers for it (up to `acme.propagation`), and `auth_gap` between clearing one and writing the next (§6.1).
#[derive(Debug)]
pub struct Dns01 {
    zone: Arc<dyn DnsZone>,
    store: Store,
    dns_zone: String,
    propagation: Duration,
    probe: Arc<dyn dns::TxtProbe>,
    gap: Duration,
    last_cleanup: Mutex<Option<tokio::time::Instant>>,
}

impl Validator for Dns01 {
    fn challenge(&self) -> &'static str {
        "dns-01"
    }

    fn parallel(&self) -> bool {
        false
    }

    fn prepare<'a>(&'a self, ident: &'a str, value: &'a str) -> BoxFuture<'a, anyhow::Result<()>> {
        async move {
            let last = *self.last_cleanup.lock().expect("cleanup lock");
            if let Some(t) = last {
                tokio::time::sleep_until(t + self.gap).await;
            }
            let host = dns::challenge_host(ident, &self.dns_zone)?;
            self.store
                .dns_add(&host, Rtype::Txt.as_str(), value, ident, now_ms())
                .await?;
            let ours = self.store.dns_values(&host, Rtype::Txt.as_str()).await?;
            dns::ensure(&*self.zone, Rtype::Txt, &host, value, &ours).await?;
            // FIX-EDGE m9: poll the authoritative servers up to `acme.propagation` (the old fixed wait). A value
            // still missing at the deadline is logged and the CA asked anyway, as before; the CA retries its own
            // lookups for a while and a failed authorization fails the attempt, which backs off.
            let fqdn = format!("{host}.{}.", self.dns_zone);
            let budget = self.propagation + Duration::from_secs(15);
            let seen = tokio::time::timeout(
                budget,
                self.probe.wait(&self.dns_zone, &fqdn, value, self.propagation),
            )
            .await;
            match seen {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(%fqdn, "challenge TXT: {e:#}"),
                Err(_) => tracing::warn!(%fqdn, "challenge TXT: authoritative poll over {budget:?}"),
            }
            Ok(())
        }
        .boxed()
    }

    fn cleanup<'a>(&'a self, ident: &'a str) -> BoxFuture<'a, anyhow::Result<()>> {
        async move {
            let host = dns::challenge_host(ident, &self.dns_zone)?;
            let ours = self.store.dns_values(&host, Rtype::Txt.as_str()).await?;
            let r = dns::remove(&*self.zone, Rtype::Txt, &host, &ours).await;
            *self.last_cleanup.lock().expect("cleanup lock") = Some(tokio::time::Instant::now());
            r?;
            self.store.dns_del(&host, Rtype::Txt.as_str()).await
        }
        .boxed()
    }
}

/// The answer of `POST …/csr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submitted {
    /// 200: the chain of this CSR.
    Chain(CertChain),
    /// 202: a job runs or waits.
    Pending(CertPending),
}

fn ms_u64(v: Option<i64>) -> u64 {
    v.and_then(|v| u64::try_from(v).ok()).unwrap_or(0)
}

fn csr_error(e: &CsrError) -> ErrorBody {
    let mut b = ErrorBody::new(e.code());
    match e {
        CsrError::Names { found, expected } => {
            b.reason = Some("names".into());
            b.message = Some(format!("SANs {found:?}, expected {expected:?}"));
        }
        other => b.reason = Some(format!("{other:?}").to_lowercase()),
    }
    b
}

fn spki_of(row: &CertRow) -> Option<SpkiHash> {
    row.csr_spki
        .as_deref()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .map(SpkiHash)
}

async fn new_job(edge: &Edge, row: &CertRow, mesh: &str, kind: &str, now: u64) -> anyhow::Result<JobRow> {
    let invited = edge.store.mesh(mesh).await?.is_some_and(|m| m.invited != 0);
    let first = edge
        .store
        .held_names(&edge.cfg.domain, mesh)
        .await?
        .iter()
        .all(|n| n.name == row.name || n.state != "valid");
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    let job = JobRow {
        id: uuid::Uuid::now_v7().to_string(),
        domain: edge.cfg.domain.clone(),
        name: row.name.clone(),
        kind: kind.into(),
        state: "queued".into(),
        class: if invited { "invited" } else { "open" }.into(),
        mesh: mesh.into(),
        net: row.net.clone(),
        first_of_mesh: i64::from(first),
        enqueued_ms: now,
        attempts: 0,
        next_at_ms: now,
        last_error: None,
    };
    edge.store.enqueue_job(&job).await?;
    Ok(job)
}

async fn pending(edge: &Edge, job: &JobRow, slotted: bool) -> anyhow::Result<Submitted> {
    if slotted || job.kind != "new" {
        return Ok(Submitted::Pending(CertPending {
            eta_s: ETA_PER_JOB_S,
            queue_pos: None,
        }));
    }
    let ca = edge.certs.get().map_or("letsencrypt", |c| c.ca.id());
    let pos = crate::queue::position(edge, ca, job, now_ms()).await?;
    Ok(Submitted::Pending(CertPending {
        eta_s: pos.map_or(ETA_PER_JOB_S, |p| p.eta_s),
        queue_pos: pos.map(|p| p.position),
    }))
}

/// `POST /v1/names/{name}/csr` (§6.2 steps 2–3): `Ok(Err(body))` is a refusal. `q.compromised` counts only with
/// `q.rotate`; it marks the name so the superseded certificate is revoked `keyCompromise` (§6.3).
pub async fn submit(
    edge: &Edge,
    mesh: &PublicKey,
    name: &str,
    der: &[u8],
    q: CsrQuery,
) -> anyhow::Result<Result<Submitted, ErrorBody>> {
    let rotate = q.rotate;
    let Some(certs) = edge.certs.get() else {
        bail!("certificates are not configured on this edge");
    };
    let domain = edge.cfg.domain.as_str();
    let mesh_s = mesh.to_string();
    let Some(row) = edge.store.cert_row(domain, name).await? else {
        return Ok(Err(ErrorBody::new(ErrorCode::NotOwner)));
    };
    if row.banned.is_some() || row.state == "banned" {
        return Ok(Err(ErrorBody::new(ErrorCode::Banned)));
    }
    if !row.held() || row.owner.as_deref() != Some(mesh_s.as_str()) {
        return Ok(Err(ErrorBody::new(ErrorCode::NotOwner)));
    }
    let info = match share_proto::check_csr(der, name, domain) {
        Ok(i) => i,
        Err(e) => return Ok(Err(csr_error(&e))),
    };
    let now = now_ms();
    // A job still open from the name's previous holder (released mid-order) is not this mesh's: its result is
    // refused by `store_chain` and revoked.
    let open = edge.store.open_job(domain, name).await?.filter(|j| j.mesh == mesh_s);
    let slotted = matches!(row.state.as_str(), "slotted" | "valid");
    // §6.5, §9.4: a CSR that will queue for a slot passes `queue_max`, the mesh's and (open meshes) the
    // network's weekly certificates and the network's queued names first.
    if !slotted && open.is_none() && row.chain_pem.is_none() {
        let invited = edge.store.mesh(&mesh_s).await?.is_some_and(|m| m.invited != 0);
        if let Err(b) = crate::limits::cert_request(edge, &mesh_s, invited, &row.net, now).await? {
            return Ok(Err(b));
        }
    }
    let answer = match row.csr.as_deref() {
        Some(stored) if stored == der => {
            if let (Some(chain), None) = (&row.chain_pem, &open) {
                return Ok(Ok(Submitted::Chain(CertChain {
                    chain_pem: chain.clone(),
                    not_after: ms_u64(row.not_after_ms),
                })));
            }
            match open {
                Some(j) => pending(edge, &j, slotted).await?,
                // A failed or dropped job earlier: the same CSR posted again starts a new one.
                None => {
                    let j = new_job(edge, &row, &mesh_s, "new", now).await?;
                    pending(edge, &j, slotted).await?
                }
            }
        }
        Some(_) if !rotate || open.as_ref().is_some_and(|j| j.state == "ordering") => {
            let held = spki_of(&row).unwrap_or(SpkiHash([0; 32]));
            return Ok(Err(ErrorBody::csr_mismatch(held)));
        }
        Some(_) => {
            edge.store.set_csr(domain, name, der, &info.spki.0, None).await?;
            // Sticky until the new chain is out: a later plain rotation still replaces the same compromised leaf.
            if q.compromised {
                edge.store.set_rotate_compromised(domain, name, true).await?;
            }
            match open {
                // A queued job reads the CSR when it runs.
                Some(j) => pending(edge, &j, slotted).await?,
                None => {
                    let kind = if row.chain_pem.is_some() { "rotate" } else { "new" };
                    let j = new_job(edge, &row, &mesh_s, kind, now).await?;
                    pending(edge, &j, slotted).await?
                }
            }
        }
        None => {
            let state = (!slotted).then_some("queued");
            if !edge.store.set_first_csr(domain, name, der, &info.spki.0, state).await? {
                // Another engine's first CSR landed since the row was read (P0-6 sync-gap race).
                let now_row = edge.store.cert_row(domain, name).await?;
                let same = now_row.as_ref().and_then(|r| r.csr.as_deref()) == Some(der);
                if !same {
                    let held = now_row.as_ref().and_then(spki_of).unwrap_or(SpkiHash([0; 32]));
                    return Ok(Err(ErrorBody::csr_mismatch(held)));
                }
            }
            let open = edge.store.open_job(domain, name).await?.filter(|j| j.mesh == mesh_s);
            let j = match open {
                Some(j) => j,
                None => new_job(edge, &row, &mesh_s, "new", now).await?,
            };
            pending(edge, &j, slotted).await?
        }
    };
    certs.wake.notify_one();
    Ok(Ok(answer))
}

/// `GET /v1/names/{name}/cert`: `Ok(Err(body))` is a refusal.
pub async fn chain(edge: &Edge, mesh: &PublicKey, name: &str) -> anyhow::Result<Result<CertChain, ErrorBody>> {
    let mesh = mesh.to_string();
    let Some(row) = edge.store.cert_row(&edge.cfg.domain, name).await? else {
        return Ok(Err(ErrorBody::new(ErrorCode::NotFound)));
    };
    if !row.held() || row.owner.as_deref() != Some(mesh.as_str()) {
        return Ok(Err(ErrorBody::new(ErrorCode::NotOwner)));
    }
    Ok(match row.chain_pem {
        Some(chain_pem) => Ok(CertChain {
            chain_pem,
            not_after: ms_u64(row.not_after_ms),
        }),
        None => Err(ErrorBody::new(ErrorCode::NotFound)),
    })
}

/// The D17 CNAMEs of `name` for `mesh`. Ownership is read again right before each write (FIX4-EDGE #14): a
/// name released or banned while the job ran gets no record, which its `retire` would never delete.
async fn ensure_records(edge: &Edge, certs: &Certs, name: &str, mesh: &str) -> anyhow::Result<()> {
    let _held = certs.lock_name(name).await;
    let target = edge.cfg.dns_target();
    for host in dns::name_hosts(name, &edge.cfg.domain, edge.cfg.dns_zone())? {
        anyhow::ensure!(still_held(edge, name, mesh).await?, "{name} is no longer held by {mesh}");
        edge.store
            .dns_add(&host, Rtype::Cname.as_str(), &target, name, now_ms())
            .await?;
        let ours = edge.store.dns_values(&host, Rtype::Cname.as_str()).await?;
        dns::ensure(&*certs.zone, Rtype::Cname, &host, &target, &ours).await?;
    }
    edge.store.set_records(&edge.cfg.domain, name, true).await
}

async fn still_held(edge: &Edge, name: &str, mesh: &str) -> anyhow::Result<bool> {
    Ok(edge
        .store
        .cert_row(&edge.cfg.domain, name)
        .await?
        .is_some_and(|r| r.held() && r.owner.as_deref() == Some(mesh)))
}

/// Delete what the ledger says the edge wrote at `host` (never another value, FIX4-EDGE #5), then forget it.
async fn remove_host(edge: &Edge, certs: &Certs, rtype: Rtype, host: &str) -> anyhow::Result<()> {
    let ours = edge.store.dns_values(host, rtype.as_str()).await?;
    if ours.is_empty() {
        return Ok(());
    }
    dns::remove(&*certs.zone, rtype, host, &ours).await?;
    edge.store.dns_del(host, rtype.as_str()).await
}

async fn remove_records(edge: &Edge, certs: &Certs, name: &str) -> anyhow::Result<()> {
    let zone = edge.cfg.dns_zone();
    // Reverse of the write order: `<n>` before `*.<n>` (see `dns::name_hosts`).
    for host in dns::name_hosts(name, &edge.cfg.domain, zone)?.iter().rev() {
        remove_host(edge, certs, Rtype::Cname, host).await?;
    }
    let txt = dns::challenge_host(&format!("{name}.{}", edge.cfg.domain), zone)?;
    remove_host(edge, certs, Rtype::Txt, &txt).await?;
    edge.store.set_records(&edge.cfg.domain, name, false).await
}

/// Release or ban (§6.3): drop the name's jobs, queue its chain for revocation (durable, retried with backoff
/// by the runner), try that revocation now, delete its records. Errors are logged; what is left in DNS is
/// removed by the next start's reconciliation, a failed revocation by the runner.
pub async fn retire(edge: &Edge, name: &str, reason: Reason) {
    let Some(certs) = edge.certs.get() else { return };
    let domain = edge.cfg.domain.as_str();
    if let Err(e) = retire_inner(edge, certs, domain, name, reason).await {
        tracing::warn!(name, "retire: {e:#}");
    }
}

async fn retire_inner(
    edge: &Edge,
    certs: &Certs,
    domain: &str,
    name: &str,
    reason: Reason,
) -> anyhow::Result<()> {
    let now = now_ms();
    // A release moved the chain already; a ban moves it here.
    edge.store.retire_chain(domain, name, reason.as_str(), now).await?;
    revoke_due(edge, certs, now).await?;
    let _held = certs.lock_name(name).await;
    match edge.store.cert_row(domain, name).await? {
        // No row: the edge never wrote records for it (a ban on an unclaimed label); nothing to delete.
        None => return Ok(()),
        // Re-claimed (by its previous owner) since the release: its jobs and records are the new holder's.
        Some(r) if r.held() => return Ok(()),
        Some(_) => {}
    }
    edge.store.drop_jobs(domain, name).await?;
    remove_records(edge, certs, name).await
}

/// Run every due revocation: `done` on success, else retried with the job backoff until `acme.max_attempts`,
/// then `failed` (logged as an error). One pass at a time.
pub async fn revoke_due(edge: &Edge, certs: &Certs, now: u64) -> anyhow::Result<()> {
    let _one = certs.revoking.lock().await;
    for r in edge.store.due_revocations(now).await? {
        let reason = Reason::parse(&r.reason).unwrap_or(Reason::CessationOfOperation);
        match dns::contain(certs.ca.revoke(&r.chain_pem, reason)).await {
            Ok(()) => edge.store.revocation_attempt(r.id, "done", now, None).await?,
            Err(e) => {
                let msg = format!("{e:#}");
                let attempts = r.attempts + 1;
                if attempts >= i64::from(edge.cfg.acme.max_attempts) {
                    tracing::error!(name = %r.name, "revocation failed for good: {msg}");
                    edge.store.revocation_attempt(r.id, "failed", now, Some(&msg)).await?;
                } else {
                    tracing::warn!(name = %r.name, attempt = attempts, "revocation: {msg}");
                    let next = now.saturating_add(u64::try_from(backoff(attempts).as_millis()).unwrap_or(0));
                    edge.store.revocation_attempt(r.id, "queued", next, Some(&msg)).await?;
                }
            }
        }
    }
    Ok(())
}

/// At start: release every lease (one runner per edge), then [`reconcile_dns`].
pub async fn reconcile(edge: &Edge) -> anyhow::Result<()> {
    if edge.certs.get().is_none() {
        return Ok(());
    }
    edge.store.reset_leases().await?;
    reconcile_dns(edge).await
}

/// Delete every ledger record that no held name needs; challenge TXTs never outlive an order. A held name's
/// CNAMEs stay even when `records` is 0 (a job that failed between ledger and flag): its next attempt
/// re-ensures them idempotently. Runs at start and hourly from the runner, between ticks, so no order of
/// this runner has a challenge TXT out while it runs (FIX4-EDGE #14).
pub async fn reconcile_dns(edge: &Edge) -> anyhow::Result<()> {
    let Some(certs) = edge.certs.get() else { return Ok(()) };
    let mut rows = edge.store.dns_all().await?;
    // `<n>` before `*.<n>`, as `remove_records` (see `dns::name_hosts`).
    rows.sort_by_key(|(host, ..)| host.starts_with("*."));
    for (host, rtype, _value, name) in rows {
        let Some(rt) = Rtype::parse(&rtype) else { continue };
        let _held = match name.as_deref() {
            Some(n) => Some(certs.lock_name(n).await),
            None => None,
        };
        let keep = match (rt, name.as_deref()) {
            (Rtype::Cname, Some(n)) => edge
                .store
                .cert_row(&edge.cfg.domain, n)
                .await?
                .is_some_and(|r| r.held()),
            _ => false,
        };
        if keep {
            continue;
        }
        match remove_host(edge, certs, rt, &host).await {
            Ok(()) => tracing::info!(%host, %rtype, "reconcile: removed orphan record"),
            Err(e) => tracing::warn!(%host, %rtype, "reconcile: {e:#}"),
        }
        if let Some(n) = name.as_deref()
            && rt == Rtype::Cname
        {
            edge.store.set_records(&edge.cfg.domain, n, false).await?;
        }
    }
    Ok(())
}

fn backoff(attempt: i64) -> Duration {
    let n = usize::try_from(attempt.max(1) - 1).unwrap_or(0);
    backon::ExponentialBuilder::default()
        .with_min_delay(Duration::from_secs(60))
        .with_max_delay(Duration::from_secs(86_400))
        .with_factor(2.0)
        .without_max_times()
        .with_jitter()
        .build()
        .nth(n)
        .unwrap_or(Duration::from_secs(86_400))
}

/// Run due jobs and, every `acme.renew_check`, the renewal scan, as of `now`. The ms until the next due job, if
/// any is waiting.
pub async fn tick(edge: &Arc<Edge>, now: u64) -> anyhow::Result<()> {
    let Some(certs) = edge.certs.get() else { return Ok(()) };
    let scan_every = u64::try_from(edge.cfg.acme.renew_check.as_millis()).unwrap_or(u64::MAX);
    let due_scan = {
        let mut last = certs.last_scan_ms.lock().expect("scan lock");
        let due = now >= last.saturating_add(scan_every);
        if due {
            *last = now;
        }
        due
    };
    if due_scan {
        renewal_scan(edge, certs, now).await?;
    }
    let lease = u64::try_from(edge.cfg.acme.job_lease.as_millis()).unwrap_or(u64::MAX);
    while let Some(job) = edge.store.lease_job(now, now.saturating_add(lease)).await? {
        match process(edge, certs, &job, now).await {
            Ok(()) => {}
            Err(e) => {
                let msg = format!("{e:#}");
                edge.ops.cert_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let limits = &edge.live().limits;
                if job.attempts >= i64::from(edge.cfg.acme.max_attempts) {
                    tracing::error!(name = %job.name, "certificate job failed for good: {msg}");
                    edge.store.finish_job(&job.id, "failed", Some(&msg)).await?;
                } else {
                    let mut wait = u64::try_from(backoff(job.attempts).as_millis()).unwrap_or(0);
                    // A name that failed `name_fail_max` times in a row pauses for `name_fail_cooldown` (§9.4).
                    if edge.ops.name_failed(&job.name, limits.name_fail_max) {
                        wait = wait.max(u64::try_from(limits.name_fail_cooldown.as_millis()).unwrap_or(u64::MAX));
                        tracing::warn!(name = %job.name, "certificate job: failure cool-down");
                    }
                    tracing::warn!(name = %job.name, attempt = job.attempts, "certificate job: {msg}");
                    edge.store.requeue_job(&job.id, now.saturating_add(wait), Some(&msg), false).await?;
                }
            }
        }
    }
    revoke_due(edge, certs, now).await?;
    crate::queue::announce(edge, certs.ca.id(), now, &certs.positions).await
}

async fn process(edge: &Arc<Edge>, certs: &Certs, job: &JobRow, now: u64) -> anyhow::Result<()> {
    let domain = edge.cfg.domain.as_str();
    let row = edge.store.cert_row(domain, &job.name).await?;
    let Some(row) = row.filter(|r| r.held() && r.owner.as_deref() == Some(job.mesh.as_str())) else {
        edge.store.finish_job(&job.id, "dropped", Some("name not held")).await?;
        return Ok(());
    };
    let Some(csr) = row.csr.clone() else {
        edge.store.finish_job(&job.id, "dropped", Some("no CSR")).await?;
        return Ok(());
    };
    // The CSR rule again before any order (§6.2): the edge never orders for a name the CSR does not match.
    if let Err(e) = share_proto::check_csr(&csr, &job.name, domain) {
        edge.store.finish_job(&job.id, "failed", Some(&format!("CSR: {e:?}"))).await?;
        return Ok(());
    }
    // Only a first certificate of a name not yet slotted draws on the slot queue; renewals and rotations
    // never do (§6.5).
    if job.kind == "new" && matches!(row.state.as_str(), "allocated" | "queued") {
        match crate::queue::try_slot(edge, certs.ca.id(), job, now).await? {
            crate::queue::Slot::Wait { pos, retry_ms } => {
                edge.store.requeue_job(&job.id, now.saturating_add(retry_ms), None, true).await?;
                edge.store.set_name_state(domain, &job.name, "queued").await?;
                if let Ok(mesh) = job.mesh.parse::<PublicKey>() {
                    for t in edge.mesh_tunnels(&mesh) {
                        t.send(Event::Queue(QueueEvent {
                            name: job.name.clone(),
                            position: pos.position,
                            eta_s: pos.eta_s,
                        }));
                    }
                }
                certs.positions.lock().expect("positions").insert(job.id.clone(), pos.position);
                return Ok(());
            }
            crate::queue::Slot::Slotted => {
                edge.store.set_name_state(domain, &job.name, "slotted").await?;
                crate::limits::count_slot(edge, &job.mesh, job.class == "invited", &job.net, now).await?;
            }
        }
    }
    // The CA's order rate (`orders_per_3h`, a bucket refilled continuously over 3 h): a job beyond it waits for
    // a token without spending an attempt.
    let per_3h = edge.cfg.acme.cas.letsencrypt.orders_per_3h.max(1);
    let orders = crate::store::BucketSpec {
        name: format!("{}_orders", certs.ca.id()),
        cap: f64::from(per_3h),
        refill_ms: (3.0 * 3_600_000.0) / f64::from(per_3h),
    };
    if let Err((_, wait_ms)) = edge.store.buckets_take(&[orders], now).await? {
        edge.store.requeue_job(&job.id, now.saturating_add(wait_ms.max(1000)), None, true).await?;
        return Ok(());
    }
    if row.records == 0 {
        ensure_records(edge, certs, &job.name, &job.mesh).await?;
    }
    let sans = share_proto::expected_sans(&job.name, domain);
    let old = row.chain_pem.clone().filter(|_| job.kind != "new");
    let issued = dns::contain(certs.ca.issue(&sans, &csr, old.as_deref(), &certs.validator)).await;
    let pem = match issued {
        Ok(p) => p,
        Err(e) => {
            // The validator clears its TXT; a panic or an error mid-write may leave one.
            let txt = dns::challenge_host(&sans[0], edge.cfg.dns_zone())?;
            if let Err(c) = remove_host(edge, certs, Rtype::Txt, &txt).await {
                tracing::warn!(name = %job.name, "challenge cleanup: {c:#}");
            }
            return Err(e);
        }
    };
    let leaf = leaf_info(&pem)?;
    let want = share_proto::check_csr(&csr, &job.name, domain)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
        .spki;
    if leaf.spki != want {
        edge.store
            .queue_revocation(domain, &job.name, &pem, Reason::CessationOfOperation.as_str(), now)
            .await?;
        bail!("issued certificate's key differs from the CSR's");
    }
    let ari = certs
        .ca
        .renewal_window(&pem)
        .await
        .map(|w| (w.start_ms, w.end_ms));
    let stored = edge
        .store
        .store_chain(domain, &job.name, &job.mesh, &want.0, certs.ca.id(), &pem, leaf.not_after_ms, ari)
        .await?;
    if !stored {
        // Released, re-claimed or re-keyed while ordering (FIX-EDGE M6): the leaf belongs to nobody.
        edge.store
            .queue_revocation(domain, &job.name, &pem, Reason::CessationOfOperation.as_str(), now)
            .await?;
        edge.store
            .finish_job(&job.id, "dropped", Some("name or CSR changed during the order"))
            .await?;
        return Ok(());
    }
    edge.store.finish_job(&job.id, "done", None).await?;
    edge.ops.name_failures.remove(&job.name);
    if let Ok(mesh) = job.mesh.parse::<PublicKey>() {
        for t in edge.mesh_tunnels(&mesh) {
            t.send(Event::Cert(CertEvent {
                name: job.name.clone(),
            }));
        }
    }
    // The old certificate goes once the new chain is out, whatever the job kind that carried the new key (a
    // rotation can land on a queued renewal): `keyCompromise` when the rotation replaced a key a removed engine
    // held, else `superseded` (§6.3). A renewal with the same key revokes nothing.
    if let Some(old) = old
        && leaf_info(&old).map_or(true, |l| l.spki != want)
    {
        let reason = if edge.store.rotate_compromised(domain, &job.name).await? {
            Reason::KeyCompromise
        } else {
            Reason::Superseded
        };
        edge.store
            .queue_revocation(domain, &job.name, &old, reason.as_str(), now)
            .await?;
        edge.store.set_rotate_compromised(domain, &job.name, false).await?;
    }
    Ok(())
}

/// Refresh ARI windows and queue renewals (§6.3): inside the ARI window, else at two thirds of the lifetime;
/// never for a name unattached for 90 days. A mesh with a live tunnel counts as attached now, and its names'
/// `last_attach_ms` is stamped (a tunnel may stay up longer than 90 days without a new `hello`).
async fn renewal_scan(edge: &Edge, certs: &Certs, now: u64) -> anyhow::Result<()> {
    let domain = edge.cfg.domain.as_str();
    for row in edge.store.valid_names(domain).await? {
        let (Some(owner), Some(chain)) = (row.owner.clone(), row.chain_pem.clone()) else {
            continue;
        };
        let live = owner
            .parse::<PublicKey>()
            .is_ok_and(|m| !edge.mesh_tunnels(&m).is_empty());
        if live {
            edge.store.touch_attached(&owner, now).await?;
        } else if now.saturating_sub(ms_u64(row.last_attach_ms)) > NO_RENEW_AFTER_MS {
            // TODO(210-M4): inactivity expiry releases and revokes; here the certificate just runs out.
            continue;
        }
        if edge.store.open_job(domain, &row.name).await?.is_some() {
            continue;
        }
        let mut ari = row
            .ari_start_ms
            .zip(row.ari_end_ms)
            .map(|(s, e)| (ms_u64(Some(s)), ms_u64(Some(e))));
        if let Some(w) = certs.ca.renewal_window(&chain).await {
            ari = Some((w.start_ms, w.end_ms));
            edge.store.set_ari(domain, &row.name, (w.start_ms, w.end_ms)).await?;
        }
        let leaf = match leaf_info(&chain) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(name = %row.name, "stored chain: {e:#}");
                continue;
            }
        };
        let two_thirds = leaf.not_before_ms + (leaf.not_after_ms.saturating_sub(leaf.not_before_ms)) * 2 / 3;
        let due = ari.map_or(now >= two_thirds, |(start, _)| now >= start);
        if due {
            new_job(edge, &row, &owner, "renew", now).await?;
        }
    }
    Ok(())
}

/// How often the runner re-runs [`reconcile_dns`] after the one at start.
const DNS_RECONCILE_EVERY: Duration = Duration::from_secs(3600);

/// The runner: reconcile once, then run due jobs whenever woken and at least every minute; DNS reconciliation
/// again every hour.
pub async fn run(edge: Arc<Edge>) {
    if let Err(e) = reconcile(&edge).await {
        tracing::warn!("DNS reconciliation: {e:#}");
    }
    let Some(certs) = edge.certs.get() else { return };
    let mut next_dns = tokio::time::Instant::now() + DNS_RECONCILE_EVERY;
    loop {
        if let Err(e) = tick(&edge, now_ms()).await {
            tracing::warn!("certificate runner: {e:#}");
        }
        if tokio::time::Instant::now() >= next_dns {
            next_dns = tokio::time::Instant::now() + DNS_RECONCILE_EVERY;
            if let Err(e) = reconcile_dns(&edge).await {
                tracing::warn!("DNS reconciliation: {e:#}");
            }
        }
        tokio::select! {
            () = certs.wake.notified() => {}
            () = tokio::time::sleep(Duration::from_secs(60)) => {}
        }
    }
}
