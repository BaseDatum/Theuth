//! Operations (210 §9.5, appendix §8.4): the counters behind `/metrics`, the brake conditions and the automatic
//! brake that flips `admission: open` to `invite`.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use share_proto::admin::{BrakeCondition, BrakeKind, BrakeStatus};
use share_proto::{Admission, ErrorBody, ErrorCode};

use crate::limits::{DAY_MS, WEEK_MS};
use crate::state::Edge;

/// Visitor bytes through the splice since start, both directions (`share_edge_bytes_total`).
#[derive(Debug, Default)]
pub struct ByteTotals {
    /// Visitor → engine.
    pub inbound: AtomicU64,
    /// Engine → visitor.
    pub outbound: AtomicU64,
}

/// The brake's memory.
#[derive(Debug, Default)]
struct BrakeMemory {
    /// Last time any condition held.
    last_held_ms: Option<u64>,
    /// When the automatic brake pulled, and why.
    pulled: Option<(u64, Vec<BrakeKind>)>,
    /// Per class bucket (`<ca>_<class>`): empty since.
    empty_since: HashMap<String, u64>,
}

/// Counters and sliding windows. Every method takes the time, so tests inject it.
#[derive(Debug, Default)]
pub struct Ops {
    /// Limit refusals since start by `code/scope/limit`.
    pub refusals: DashMap<String, AtomicU64>,
    /// Certificate job attempts that failed (retried or for good).
    pub cert_failures: AtomicU64,
    /// Splice bytes.
    pub bytes: Arc<ByteTotals>,
    /// Last `edge_full` answer.
    pub last_edge_full_ms: AtomicU64,
    /// Failed attempts per name since its last cool-down (`limits.name_fail_max`).
    pub name_failures: DashMap<String, u32>,
    refusal_times: Mutex<VecDeque<u64>>,
    mesh_times: Mutex<VecDeque<u64>>,
    brake: Mutex<BrakeMemory>,
}

const MINUTE_MS: u64 = 60_000;

fn push(q: &Mutex<VecDeque<u64>>, now_ms: u64) {
    let mut q = q.lock().expect("window lock");
    q.push_back(now_ms);
    trim(&mut q, now_ms);
}

fn trim(q: &mut VecDeque<u64>, now_ms: u64) {
    while q.front().is_some_and(|t| now_ms.saturating_sub(*t) >= MINUTE_MS) {
        q.pop_front();
    }
}

fn count(q: &Mutex<VecDeque<u64>>, now_ms: u64) -> u32 {
    let mut q = q.lock().expect("window lock");
    trim(&mut q, now_ms);
    u32::try_from(q.len()).unwrap_or(u32::MAX)
}

/// The answers the brake and `share_edge_refusals_total` count as limit refusals: the quota codes, and
/// `rate_limited` when it names its scope (the availability lookup limit, LAUNCH-EDGE; `/v1/info` floods carry
/// none and stay uncounted).
pub fn is_limit_refusal(b: &ErrorBody) -> bool {
    match b.code {
        ErrorCode::NetworkQuota
        | ErrorCode::Paced
        | ErrorCode::Quota
        | ErrorCode::EnginesQuota
        | ErrorCode::EdgeFull => true,
        ErrorCode::RateLimited => b.scope.is_some(),
        _ => false,
    }
}

fn label<T: serde::Serialize>(v: Option<T>) -> String {
    v.and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "none".into())
}

impl Ops {
    /// Count an error answer; only limit refusals are kept.
    pub fn refused(&self, b: &ErrorBody, now_ms: u64) {
        if !is_limit_refusal(b) {
            return;
        }
        let key = format!("{}/{}/{}", b.code, label(b.scope), label(b.limit));
        self.refusals
            .entry(key)
            .or_default()
            .fetch_add(1, Ordering::Relaxed);
        if b.code == ErrorCode::EdgeFull {
            self.last_edge_full_ms.store(now_ms, Ordering::Relaxed);
        }
        push(&self.refusal_times, now_ms);
    }

    /// A mesh first seen.
    pub fn new_mesh(&self, now_ms: u64) {
        push(&self.mesh_times, now_ms);
    }

    /// Limit refusals in the last minute.
    pub fn refusals_per_min(&self, now_ms: u64) -> u32 {
        count(&self.refusal_times, now_ms)
    }

    /// Meshes first seen in the last minute.
    pub fn meshes_per_min(&self, now_ms: u64) -> u32 {
        count(&self.mesh_times, now_ms)
    }

    /// A failed attempt of `name`'s job: true when it reached `max` (the counter restarts).
    pub fn name_failed(&self, name: &str, max: u32) -> bool {
        let mut n = self.name_failures.entry(name.to_string()).or_insert(0);
        *n += 1;
        if *n >= max.max(1) {
            *n = 0;
            true
        } else {
            false
        }
    }
}

fn cond(kind: BrakeKind, holds: bool, detail: Option<String>) -> BrakeCondition {
    BrakeCondition { kind, holds, detail }
}

/// Which §9.5 conditions hold at `now_ms`. Search Console and Safe Browsing are watched outside the edge
/// (`TODO(210-M4)`: no lookup client here); they always report `holds: false` with a detail saying so.
pub async fn brake_status(edge: &Edge, now_ms: u64) -> anyhow::Result<BrakeStatus> {
    let live = edge.live();
    let b = &live.brake;
    let mut conds = Vec::new();
    let last_full = edge.ops.last_edge_full_ms.load(Ordering::Relaxed);
    let full_recent = last_full != 0 && now_ms.saturating_sub(last_full) < DAY_MS;
    conds.push(cond(
        BrakeKind::EdgeFull,
        edge.full() || full_recent,
        (edge.full() || last_full != 0).then(|| {
            if edge.full() {
                "names_max reached now".into()
            } else {
                format!("last edge_full answer at {last_full} ms")
            }
        }),
    ));
    let (eta, empty) = match edge.certs.get() {
        Some(certs) => {
            let ca = certs.ca.id();
            let jobs = edge.store.slot_jobs(&edge.cfg.domain).await?;
            let mut eta = 0u64;
            let mut empty = Vec::new();
            for class in ["invited", "open"] {
                let v = crate::queue::view(edge, ca, class, &jobs, now_ms).await?;
                if let Some(last) = v.ids.last() {
                    eta = eta.max(v.pos(last).map_or(0, |p| p.eta_s));
                }
                let key = format!("{ca}_{class}");
                let tokens = edge
                    .store
                    .bucket_tokens(&crate::queue::bucket(edge, ca, class), now_ms)
                    .await?;
                let mut m = edge.ops.brake.lock().expect("brake lock");
                if tokens < 1.0 {
                    let since = *m.empty_since.entry(key.clone()).or_insert(now_ms);
                    empty.push((key, since));
                } else {
                    m.empty_since.remove(&key);
                }
            }
            (eta, empty)
        }
        None => (0, Vec::new()),
    };
    let eta_max = b.queue_eta_hours.saturating_mul(3600);
    conds.push(cond(
        BrakeKind::QueueEta,
        eta_max > 0 && eta > eta_max,
        Some(format!("longest queue ETA {}h (brake above {}h)", eta / 3600, b.queue_eta_hours)),
    ));
    let week_empty: Vec<_> = empty
        .iter()
        .filter(|(_, since)| now_ms.saturating_sub(*since) >= WEEK_MS)
        .collect();
    conds.push(cond(
        BrakeKind::BucketEmpty,
        !week_empty.is_empty(),
        (!empty.is_empty()).then(|| {
            empty
                .iter()
                .map(|(k, s)| format!("{k} empty since {s} ms"))
                .collect::<Vec<_>>()
                .join(", ")
        }),
    ));
    for k in [BrakeKind::SearchConsole, BrakeKind::SafeBrowsing] {
        conds.push(cond(k, false, Some("watched outside the edge; not reported here".into())));
    }
    let refusals = edge.ops.refusals_per_min(now_ms);
    let meshes = edge.ops.meshes_per_min(now_ms);
    let spike_r = b.refusals_per_min > 0 && refusals >= b.refusals_per_min;
    let spike_m = b.new_meshes_per_min > 0 && meshes >= b.new_meshes_per_min;
    conds.push(cond(
        BrakeKind::AbuseSpike,
        spike_r || spike_m,
        Some(format!(
            "limit refusals {refusals}/min (brake at {}), new meshes {meshes}/min (brake at {})",
            b.refusals_per_min, b.new_meshes_per_min
        )),
    ));
    let pull = conds.iter().any(|c| c.holds);
    let clear_days = {
        let mut m = edge.ops.brake.lock().expect("brake lock");
        if pull {
            m.last_held_ms = Some(now_ms);
        }
        m.last_held_ms
            .map(|t| u32::try_from(now_ms.saturating_sub(t) / DAY_MS).unwrap_or(u32::MAX))
    };
    Ok(BrakeStatus {
        admission: edge.admission(),
        pull,
        conditions: conds,
        clear_days,
    })
}

/// When the automatic brake last pulled, and on which conditions.
pub fn pulled(edge: &Edge) -> Option<(u64, Vec<BrakeKind>)> {
    edge.ops.brake.lock().expect("brake lock").pulled.clone()
}

/// One brake pass: with `brake.auto` and `admission: open`, a holding condition flips admission to `invite`
/// (admitted meshes keep working; §9.5). Returns the status it acted on.
pub async fn brake_tick(edge: &Edge, now_ms: u64) -> anyhow::Result<BrakeStatus> {
    let mut st = brake_status(edge, now_ms).await?;
    if st.pull && edge.live().brake.auto && edge.admission() == Admission::Open {
        let kinds: Vec<BrakeKind> = st.conditions.iter().filter(|c| c.holds).map(|c| c.kind).collect();
        *edge.admission.write().expect("admission lock") = Admission::Invite;
        tracing::warn!(?kinds, "brake: admission open → invite");
        let args = serde_json::json!({ "mode": "invite", "kinds": kinds }).to_string();
        if let Err(e) = edge.store.audit("brake", &args, Some("auto"), now_ms).await {
            tracing::warn!("brake audit: {e:#}");
        }
        edge.ops.brake.lock().expect("brake lock").pulled = Some((now_ms, kinds));
        st.admission = Admission::Invite;
    }
    Ok(st)
}

/// The brake loop: every 10 s.
pub async fn run_brake(edge: Arc<Edge>) {
    let mut every = tokio::time::interval(std::time::Duration::from_secs(10));
    loop {
        every.tick().await;
        if let Err(e) = brake_tick(&edge, crate::state::now_ms()).await {
            tracing::warn!("brake: {e:#}");
        }
    }
}

/// `/metrics` in the Prometheus text format (appendix §8.4, §9.5's monitoring lines).
pub async fn render(edge: &Edge, now_ms: u64) -> String {
    let mut m = String::new();
    let live = edge.live();
    let g = |m: &mut String, name: &str, v: u64| {
        let _ = writeln!(m, "share_edge_{name} {v}");
    };
    g(&mut m, "tunnels", edge.tunnels.len() as u64);
    g(&mut m, "routes", edge.routes.len() as u64);
    g(
        &mut m,
        "conns",
        (edge.cfg.limits.global_conns as usize).saturating_sub(edge.visitors.available_permits()) as u64,
    );
    g(&mut m, "conn_log_dropped_total", edge.conn_log.dropped.load(Ordering::Relaxed));
    let _ = writeln!(m, "share_edge_bytes_total{{dir=\"in\"}} {}", edge.ops.bytes.inbound.load(Ordering::Relaxed));
    let _ = writeln!(m, "share_edge_bytes_total{{dir=\"out\"}} {}", edge.ops.bytes.outbound.load(Ordering::Relaxed));
    g(&mut m, "egress_month_bytes", edge.meter.month_bytes.load(Ordering::Relaxed));
    let throttled = edge.meter.throttled.load(Ordering::Relaxed);
    g(&mut m, "throttle_engaged", u64::from(throttled));
    let throttled_tunnels = if throttled {
        edge.tunnels.iter().filter(|t| !t.invited.load(Ordering::Relaxed) && t.mesh().is_some()).count()
    } else {
        0
    };
    g(&mut m, "throttled_tunnels", throttled_tunnels as u64);
    g(&mut m, "edge_full", u64::from(edge.full()));
    g(&mut m, "names_max", u64::from(live.limits.names_max));
    g(&mut m, "queue_max", u64::from(live.queue_max));
    match edge.store.slotted_count(&edge.cfg.domain).await {
        Ok(n) => g(&mut m, "names_slotted", n),
        Err(e) => tracing::warn!("metrics: {e:#}"),
    }
    if let Some(certs) = edge.certs.get() {
        let ca = certs.ca.id();
        if let Ok(jobs) = edge.store.slot_jobs(&edge.cfg.domain).await {
            for class in ["invited", "open"] {
                let depth = jobs.iter().filter(|j| j.class == class).count();
                let _ = writeln!(m, "share_edge_queue_depth{{class=\"{class}\"}} {depth}");
                if let Ok(t) = edge.store.bucket_tokens(&crate::queue::bucket(edge, ca, class), now_ms).await {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let whole = t.max(0.0).floor() as u64;
                    let _ = writeln!(m, "share_edge_slots_left{{ca=\"{ca}\",class=\"{class}\"}} {whole}");
                }
            }
        }
    }
    g(&mut m, "cert_job_failures_total", edge.ops.cert_failures.load(Ordering::Relaxed));
    let mut refusals: Vec<(String, u64)> = edge
        .ops
        .refusals
        .iter()
        .map(|r| (r.key().clone(), r.value().load(Ordering::Relaxed)))
        .collect();
    refusals.sort();
    for (k, v) in refusals {
        let mut p = k.splitn(3, '/');
        let (code, scope, limit) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""));
        let _ = writeln!(
            m,
            "share_edge_refusals_total{{code=\"{code}\",scope=\"{scope}\",limit=\"{limit}\"}} {v}"
        );
    }
    g(&mut m, "refusals_per_min", u64::from(edge.ops.refusals_per_min(now_ms)));
    g(&mut m, "new_meshes_per_min", u64::from(edge.ops.meshes_per_min(now_ms)));
    let adm = edge.admission();
    for a in [Admission::Open, Admission::Invite, Admission::Closed] {
        let _ = writeln!(m, "share_edge_admission{{mode=\"{}\"}} {}", label(Some(a)), u64::from(a == adm));
    }
    g(&mut m, "brake_pulled", u64::from(pulled(edge).is_some()));
    if let Some(k) = edge.k8s.get() {
        g(&mut m, "k8s_errors_total", k.errors.load(Ordering::Relaxed));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use share_proto::{LimitKind, LimitScope};

    #[test]
    fn windows_slide_and_limit_refusals_only_count() {
        let o = Ops::default();
        let t0 = 1_000_000;
        o.refused(&ErrorBody::limited(ErrorCode::NetworkQuota, LimitScope::Net, LimitKind::Meshes), t0);
        o.refused(&ErrorBody::new(ErrorCode::Banned), t0);
        o.refused(&ErrorBody::limited(ErrorCode::Paced, LimitScope::Global, LimitKind::Names), t0 + 30_000);
        assert_eq!(o.refusals_per_min(t0 + 30_000), 2);
        assert_eq!(o.refusals_per_min(t0 + 60_000), 1);
        assert_eq!(o.refusals_per_min(t0 + 90_000), 0);
        assert_eq!(o.refusals.len(), 2);
        assert_eq!(
            o.refusals.get("network_quota/net/meshes").map(|v| v.load(Ordering::Relaxed)),
            Some(1)
        );
        o.new_mesh(t0);
        assert_eq!(o.meshes_per_min(t0 + 59_999), 1);
        assert!(!o.name_failed("a", 2));
        assert!(o.name_failed("a", 2));
        assert!(!o.name_failed("a", 2));
    }
}
