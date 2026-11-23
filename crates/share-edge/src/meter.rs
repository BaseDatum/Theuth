//! Byte metering and shaping (210 §9.6): per-tunnel byte shaping at `tunnel_mbit`, the per-host `stats` event
//! every 30 s, the fold of tunnel counters into per-mesh and per-network day totals every second, the daily
//! budgets (`kill {budget}` and route refusal until the UTC day rolls), the month's egress against its pace
//! and the egress throttle of non-invited tunnels (`throttle` event, `welcome.throttled`), and the flush into
//! `usage` and `counters` every 60 s so the totals survive a restart.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashmap::DashMap;
use share_proto::events::StatsEvent;
use share_proto::{Event, KillReason, ThrottleEvent};
use tokio::time::Instant;

use crate::limits::{day_start, month_bounds, tiers};
use crate::state::{Edge, Tunnel};
use crate::store::CounterCheck;

/// Largest shaping burst (§9.6: 64 MiB).
pub const MAX_BURST: u64 = 64 << 20;

/// Bytes per second of `mbit` Mbit/s.
pub fn mbit_bytes(mbit: u32) -> u64 {
    u64::from(mbit) * 1_000_000 / 8
}

/// A byte token bucket shared by every visitor stream of a tunnel; the rate can change live (throttle).
/// Burst = min(64 MiB, 10 s of the rate). Rate 0 = unshaped.
#[derive(Debug)]
pub struct Shaper {
    rate: AtomicU64,
    state: Mutex<(f64, Instant)>,
}

impl Shaper {
    /// A full bucket at `rate` bytes/s.
    pub fn new(rate: u64) -> Shaper {
        Shaper {
            rate: AtomicU64::new(rate),
            #[allow(clippy::cast_precision_loss)]
            state: Mutex::new((Self::burst_of(rate) as f64, Instant::now())),
        }
    }

    fn burst_of(rate: u64) -> u64 {
        MAX_BURST.min(rate.saturating_mul(10))
    }

    /// Bytes per second now.
    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    /// Re-rate; the bucket keeps its tokens up to the new burst (an unshaped tunnel starts with a full one).
    pub fn set_rate(&self, rate: u64) {
        let was = self.rate.swap(rate, Ordering::Relaxed);
        let mut s = self.state.lock().expect("shaper");
        #[allow(clippy::cast_precision_loss)]
        let burst = Self::burst_of(rate) as f64;
        if was == 0 {
            *s = (burst, Instant::now());
        } else {
            s.0 = s.0.min(burst);
        }
    }

    /// Charge `n` bytes at `now`; the delay before the next transfer (zero while within the burst).
    pub fn charge(&self, n: usize, now: Instant) -> Duration {
        let rate = self.rate();
        if rate == 0 {
            return Duration::ZERO;
        }
        #[allow(clippy::cast_precision_loss)]
        let (rate_f, burst) = (rate as f64, Self::burst_of(rate) as f64);
        let mut s = self.state.lock().expect("shaper");
        let elapsed = now.saturating_duration_since(s.1).as_secs_f64();
        #[allow(clippy::cast_precision_loss)]
        let tokens = (s.0 + elapsed * rate_f).min(burst) - n as f64;
        *s = (tokens, now);
        if tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-tokens / rate_f)
        }
    }
}

/// Per-host counters for the `stats` event, reset at each emission.
#[derive(Debug, Default)]
pub struct HostStats {
    /// Visitor connections in the period.
    pub conns: AtomicU64,
    /// Visitor → engine bytes in the period.
    pub bytes_in: AtomicU64,
    /// Engine → visitor bytes in the period.
    pub bytes_out: AtomicU64,
    /// Open visitor connections now.
    pub open: AtomicU64,
    /// Distinct visitor addresses (/64) in the period.
    pub visitors: Mutex<HashSet<IpAddr>>,
    /// `first_live_ms` was stamped from this route (no further store writes).
    pub live: AtomicBool,
}

impl HostStats {
    /// The period's event, counters reset (`open` stays). `None` when nothing happened and nothing is open.
    pub fn take(&self, host: &str, period_s: u64) -> Option<StatsEvent> {
        let conns = self.conns.swap(0, Ordering::Relaxed);
        let bytes_in = self.bytes_in.swap(0, Ordering::Relaxed);
        let bytes_out = self.bytes_out.swap(0, Ordering::Relaxed);
        let open = self.open.load(Ordering::Relaxed);
        let visitors = {
            let mut v = self.visitors.lock().expect("visitors");
            let n = v.len() as u64;
            v.clear();
            n
        };
        (conns + bytes_in + bytes_out + open > 0).then(|| StatsEvent {
            host: host.to_string(),
            conns,
            bytes: bytes_in + bytes_out,
            bytes_in,
            bytes_out,
            visitors,
            open,
            period_s,
        })
    }
}

/// The edge's byte totals.
#[derive(Debug, Default)]
pub struct Meter {
    day: AtomicU64,
    /// Today's bytes per mesh.
    pub mesh_day: DashMap<String, u64>,
    /// Today's bytes per network tier (`<scope>:<key>` → (bytes, factor)).
    pub net_day: DashMap<String, (u64, u64)>,
    /// Keys over today's budget (`mesh:<id>`, or a network tier key).
    pub over: DashMap<String, ()>,
    pending: DashMap<(String, String, bool), u64>,
    month_start: AtomicU64,
    /// This month's bytes, both directions (all of the host's egress at the splice).
    pub month_bytes: AtomicU64,
    month_unflushed: AtomicU64,
    /// Egress over pace: non-invited tunnels run at `throttle_mbit`.
    pub throttled: AtomicBool,
}

fn tier_key(t: &crate::limits::Tier) -> String {
    format!("{:?}:{}", t.scope, t.key)
}

impl Meter {
    /// Load today's totals and the month's egress from the store (after a restart).
    pub async fn load(edge: &Edge, now_ms: u64) -> anyhow::Result<()> {
        let m = &edge.meter;
        let day = day_start(now_ms);
        m.day.store(day, Ordering::Relaxed);
        for (mesh, net, bytes) in edge.store.usage_day(day).await? {
            let b = u64::try_from(bytes).unwrap_or(0);
            *m.mesh_day.entry(mesh).or_insert(0) += b;
            for t in tiers(&net, &edge.cfg.limits, &edge.asn) {
                m.net_day.entry(tier_key(&t)).or_insert((0, t.factor)).0 += b;
            }
        }
        let (start, _) = month_bounds(now_ms);
        m.month_start.store(start, Ordering::Relaxed);
        let b = edge.store.counter("egress", "all", "month", start).await?;
        m.month_bytes.store(b, Ordering::Relaxed);
        Ok(())
    }

    /// Fold one tunnel's new bytes into the mesh, network and month totals (also called at detach).
    pub fn fold_tunnel(&self, edge: &Edge, t: &Tunnel) {
        let total = t.bytes.load(Ordering::Relaxed);
        let delta = total.saturating_sub(t.folded.swap(total, Ordering::Relaxed));
        let Some(mesh) = t.mesh() else { return };
        if delta == 0 {
            return;
        }
        let mesh = mesh.to_string();
        let invited = t.invited.load(Ordering::Relaxed);
        *self.pending.entry((mesh.clone(), t.net.clone(), invited)).or_insert(0) += delta;
        *self.mesh_day.entry(mesh).or_insert(0) += delta;
        for tier in tiers(&t.net, &edge.cfg.limits, &edge.asn) {
            self.net_day.entry(tier_key(&tier)).or_insert((0, tier.factor)).0 += delta;
        }
        self.month_bytes.fetch_add(delta, Ordering::Relaxed);
        self.month_unflushed.fetch_add(delta, Ordering::Relaxed);
    }

    /// Whether a mesh or a tier of `net` is over today's budget (new routes refused).
    pub fn over_budget(&self, edge: &Edge, mesh: &str, net: &str) -> bool {
        self.over.contains_key(&format!("mesh:{mesh}"))
            || tiers(net, &edge.cfg.limits, &edge.asn)
                .iter()
                .any(|t| self.over.contains_key(&tier_key(t)))
    }

    /// The per-tunnel rate a tunnel of an (un)invited mesh gets now.
    pub fn rate_for(&self, edge: &Edge, invited: bool) -> u64 {
        let l = &edge.cfg.limits;
        if !invited && self.throttled.load(Ordering::Relaxed) {
            mbit_bytes(l.throttle_mbit)
        } else {
            mbit_bytes(l.tunnel_mbit)
        }
    }
}

/// One second of metering: day roll, fold, budgets, month pace and throttle.
pub fn tick(edge: &Edge, now_ms: u64) {
    let m = &edge.meter;
    let l = &edge.cfg.limits;
    let day = day_start(now_ms);
    if m.day.swap(day, Ordering::Relaxed) != day {
        m.mesh_day.clear();
        m.net_day.clear();
        m.over.clear();
    }
    let (start, len) = month_bounds(now_ms);
    if m.month_start.swap(start, Ordering::Relaxed) != start {
        m.month_bytes.store(0, Ordering::Relaxed);
    }
    let tunnels: Vec<Arc<Tunnel>> = edge.tunnels.iter().map(|t| t.value().clone()).collect();
    for t in &tunnels {
        m.fold_tunnel(edge, t);
    }
    // Budgets: a mesh or network over its day's bytes gets `kill {budget}` on every tunnel, once a day.
    let over_mesh: Vec<String> = m
        .mesh_day
        .iter()
        .filter(|e| *e.value() > l.bytes_per_mesh_day)
        .map(|e| e.key().clone())
        .filter(|k| m.over.insert(format!("mesh:{k}"), ()).is_none())
        .collect();
    let over_net: Vec<String> = m
        .net_day
        .iter()
        .filter(|e| e.value().0 > l.bytes_per_net_day.saturating_mul(e.value().1))
        .map(|e| e.key().clone())
        .filter(|k| m.over.insert(k.clone(), ()).is_none())
        .collect();
    for t in &tunnels {
        let mesh_hit = t.mesh().is_some_and(|ms| over_mesh.contains(&ms.to_string()));
        let net_hit = !over_net.is_empty()
            && tiers(&t.net, l, &edge.asn)
                .iter()
                .any(|x| over_net.contains(&tier_key(x)));
        if mesh_hit || net_hit {
            tracing::info!(engine = %t.engine, "daily byte budget reached");
            edge.detach(t);
            t.kill(None, KillReason::Budget);
        }
    }
    // Pace: the month's allowance × the elapsed fraction of the month.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let pace = (l.egress_month as f64 * (now_ms.saturating_sub(start) as f64 / len as f64)) as u64;
    let over = m.month_bytes.load(Ordering::Relaxed) > pace;
    if m.throttled.swap(over, Ordering::Relaxed) != over {
        tracing::info!(throttled = over, "egress pace");
        let mbit = if over { l.throttle_mbit } else { l.tunnel_mbit };
        for t in tunnels.iter().filter(|t| !t.invited.load(Ordering::Relaxed)) {
            t.shaper.set_rate(mbit_bytes(mbit));
            t.send(Event::Throttle(ThrottleEvent { throttled: over, mbit }));
        }
    }
}

/// Write the pending usage rows and the month's egress counter (every 60 s and at shutdown).
pub async fn flush(edge: &Edge, now_ms: u64) -> anyhow::Result<()> {
    let m = &edge.meter;
    let day = day_start(now_ms);
    let keys: Vec<(String, String, bool)> = m.pending.iter().map(|e| e.key().clone()).collect();
    for k in keys {
        if let Some((k, bytes)) = m.pending.remove(&k) {
            edge.store.usage_add(day, &k.0, &k.1, k.2, bytes).await?;
        }
    }
    let n = m.month_unflushed.swap(0, Ordering::Relaxed);
    if n > 0 {
        let c = CounterCheck {
            scope: "egress".into(),
            key: "all".into(),
            period: "month",
            start_ms: m.month_start.load(Ordering::Relaxed),
            limit: u64::MAX,
        };
        edge.store.counters_add(&[c], n).await?;
    }
    Ok(())
}

/// The `stats` event of every route's host to its tunnel (every `STATS_PERIOD_S`).
pub fn stats(edge: &Edge) {
    let period = share_proto::limits::STATS_PERIOD_S;
    for r in &edge.routes {
        if let Some(ev) = r.stats.take(r.key(), period) {
            r.tunnel.send(Event::Stats(ev));
        }
    }
}

/// The metering loop: tick every second, stats every 30 s, flush and housekeeping every 60 s.
pub async fn run(edge: Arc<Edge>) {
    let mut n: u64 = 0;
    let mut every = tokio::time::interval(Duration::from_secs(1));
    loop {
        every.tick().await;
        n += 1;
        let now = crate::state::now_ms();
        tick(&edge, now);
        if n.is_multiple_of(share_proto::limits::STATS_PERIOD_S) {
            stats(&edge);
        }
        if n.is_multiple_of(60) {
            if let Err(e) = flush(&edge, now).await {
                tracing::warn!("meter flush: {e:#}");
            }
            if let Err(e) = crate::queue::housekeeping(&edge, now).await {
                tracing::warn!("housekeeping: {e:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn shaper_paces_after_the_burst_and_re_rates() {
        let t0 = Instant::now();
        let s = Shaper::new(1_000_000); // 1 MB/s, burst 10 MB
        assert_eq!(s.charge(10_000_000, t0), Duration::ZERO);
        // 1 MB over the burst: one second.
        assert_eq!(s.charge(1_000_000, t0), Duration::from_secs(1));
        // Throttled to 100 kB/s: the burst shrinks to 1 MB; the 1 MB debt drains at the new rate (10 s).
        s.set_rate(100_000);
        assert_eq!(s.charge(0, t0 + Duration::from_secs(9)), Duration::from_secs(1));
        assert_eq!(s.charge(200_000, t0 + Duration::from_secs(10)), Duration::from_secs(2));
        // Unshaped.
        s.set_rate(0);
        assert_eq!(s.charge(usize::MAX, t0), Duration::ZERO);
    }

    #[test]
    fn host_stats_reset_each_period() {
        let h = HostStats::default();
        assert!(h.take("a.tun.zip", 30).is_none());
        h.conns.fetch_add(2, Ordering::Relaxed);
        h.bytes_in.fetch_add(10, Ordering::Relaxed);
        h.bytes_out.fetch_add(5, Ordering::Relaxed);
        h.open.fetch_add(1, Ordering::Relaxed);
        h.visitors.lock().unwrap().insert("192.0.2.1".parse().unwrap());
        let e = h.take("a.tun.zip", 30).unwrap();
        assert_eq!((e.conns, e.bytes, e.bytes_in, e.bytes_out, e.visitors, e.open, e.period_s), (2, 15, 10, 5, 1, 1, 30));
        // Next period: only the open connection.
        let e = h.take("a.tun.zip", 30).unwrap();
        assert_eq!((e.conns, e.bytes, e.open), (0, 0, 1));
    }
}
