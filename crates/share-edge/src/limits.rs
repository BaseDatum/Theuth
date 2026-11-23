//! Admission limits (210 §9.4): network tiers (IPv4 address and /24; IPv6 /64, /48 and /32; origin ASN), the
//! durable day/week counters and token buckets behind new meshes, claims and certificate requests, the paced
//! relayed-newcomer and open-names buckets, and the `edge_full` state. Every refusal carries `scope` and
//! `limit` (CONTRACT2). Every function takes `now_ms`, so tests inject time.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use anyhow::Context as _;
use share_proto::{ErrorBody, ErrorCode, LimitKind, LimitScope, NameKind};

use crate::config::Limits;
use crate::state::Edge;
use crate::store::{BucketSpec, CounterCheck};

/// One day, ms.
pub const DAY_MS: u64 = 86_400_000;
/// One week, ms.
pub const WEEK_MS: u64 = 7 * DAY_MS;
const HOUR_MS: u64 = 3_600_000;

/// Start of the UTC day of `now_ms`.
pub fn day_start(now_ms: u64) -> u64 {
    now_ms - now_ms % DAY_MS
}

/// Start of the fixed 7-day window of `now_ms` (Unix epoch aligned).
pub fn week_start(now_ms: u64) -> u64 {
    now_ms - now_ms % WEEK_MS
}

/// Start and length of the UTC month of `now_ms`, ms.
pub fn month_bounds(now_ms: u64) -> (u64, u64) {
    let secs = i64::try_from(now_ms / 1000).unwrap_or(0);
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp(secs) else {
        return (now_ms, 30 * DAY_MS);
    };
    let first = t.date().replace_day(1).expect("day 1");
    let next = first
        .checked_add(time::Duration::days(i64::from(first.month().length(first.year()))))
        .expect("next month");
    let ms_of = |d: time::Date| u64::try_from(d.midnight().assume_utc().unix_timestamp()).unwrap_or(0) * 1000;
    let (a, b) = (ms_of(first), ms_of(next));
    (a, b - a)
}

/// One tier of a network: the scope reported in a refusal, the key counted, the limit multiplier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier {
    /// `net`, `net_wide`, `net32` or `asn`.
    pub scope: LimitScope,
    /// The counted key, e.g. `203.0.113.0/24`, `as64500`.
    pub key: String,
    /// Multiplier of the per-network limit.
    pub factor: u64,
}

/// Whether a tunnel network is a relay (`relay:<url>`): such meshes use the relayed-newcomer bucket.
pub fn is_relayed(net: &str) -> bool {
    net.starts_with("relay:")
}

fn scope_tag(s: LimitScope) -> &'static str {
    match s {
        LimitScope::Net => "net",
        LimitScope::NetWide => "wide",
        LimitScope::Net32 => "net32",
        LimitScope::Asn => "asn",
        _ => "other",
    }
}

/// The wide key (IPv4 /24, IPv6 /48) of a network, or the network itself when it is not an address; the slot
/// queue's round-robin key (§6.5).
pub fn wide_key(net: &str) -> String {
    match net.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            let o = v4.octets();
            format!("{}/24", Ipv4Addr::new(o[0], o[1], o[2], 0))
        }
        Ok(IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{}/48", Ipv6Addr::new(s[0], s[1], s[2], 0, 0, 0, 0, 0))
        }
        Err(_) => net.to_string(),
    }
}

/// The tiers of a tunnel network (§9.4): none for a relay or an unknown path.
pub fn tiers(net: &str, l: &Limits, asn: &AsnDb) -> Vec<Tier> {
    let Ok(ip) = net.parse::<IpAddr>() else {
        return Vec::new();
    };
    let wide = u64::from(l.wide_net_factor.max(1));
    let mut v = vec![
        Tier {
            scope: LimitScope::Net,
            key: net.to_string(),
            factor: 1,
        },
        Tier {
            scope: LimitScope::NetWide,
            key: wide_key(net),
            factor: wide,
        },
    ];
    if let IpAddr::V6(v6) = ip {
        let s = v6.segments();
        v.push(Tier {
            scope: LimitScope::Net32,
            key: format!("{}/32", Ipv6Addr::new(s[0], s[1], 0, 0, 0, 0, 0, 0)),
            factor: u64::from(l.v6_32_factor.max(1)),
        });
    }
    if let Some(a) = asn.lookup(ip) {
        v.push(Tier {
            scope: LimitScope::Asn,
            key: format!("as{a}"),
            factor: u64::from(l.asn_factor.max(1)),
        });
    }
    v
}

/// Whether network `net` falls in tier `t` (for counting rows that store only the network).
pub fn in_tier(net: &str, t: &Tier, l: &Limits, asn: &AsnDb) -> bool {
    tiers(net, l, asn).iter().any(|x| x.scope == t.scope && x.key == t.key)
}

/// Origin-ASN ranges from a local file (§9.4): iptoasn.com's TSV (`start end asn …`) or `cidr asn` lines;
/// ASN 0 (not routed) and unparsable lines are skipped. Empty = the ASN tier is off.
#[derive(Debug, Clone, Default)]
pub struct AsnDb {
    /// `(start, end, asn)` over IPv6 space (IPv4 as v4-mapped), sorted by start.
    ranges: Vec<(u128, u128, u32)>,
}

fn ip_u128(ip: IpAddr) -> u128 {
    match ip.to_canonical() {
        IpAddr::V4(v4) => u128::from(v4.to_ipv6_mapped()),
        IpAddr::V6(v6) => u128::from(v6),
    }
}

impl AsnDb {
    /// Load `path`; `None` = off.
    pub fn load(path: Option<&Path>) -> anyhow::Result<AsnDb> {
        let Some(p) = path else { return Ok(AsnDb::default()) };
        if !p.exists() {
            tracing::warn!(path = %p.display(), "limits.asn_db missing: ASN tier off");
            return Ok(AsnDb::default());
        }
        let text = std::fs::read_to_string(p).with_context(|| format!("asn_db {}", p.display()))?;
        Ok(AsnDb::parse(&text))
    }

    /// Parse the file's text.
    pub fn parse(text: &str) -> AsnDb {
        let mut ranges = Vec::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            let parsed = match f.as_slice() {
                [cidr, asn, ..] if cidr.contains('/') => cidr
                    .parse::<crate::config::Cidr>()
                    .ok()
                    .and_then(|c| Some((c.range(), asn.trim_start_matches("AS").parse::<u32>().ok()?))),
                [start, end, asn, ..] => (|| {
                    let s = ip_u128(start.parse().ok()?);
                    let e = ip_u128(end.parse().ok()?);
                    Some(((s, e), asn.parse::<u32>().ok()?))
                })(),
                _ => None,
            };
            if let Some(((s, e), a)) = parsed
                && a != 0
            {
                ranges.push((s, e, a));
            }
        }
        ranges.sort_unstable();
        AsnDb { ranges }
    }

    /// The origin ASN of `ip`.
    pub fn lookup(&self, ip: IpAddr) -> Option<u32> {
        let x = ip_u128(ip);
        let i = self.ranges.partition_point(|r| r.0 <= x);
        let r = self.ranges.get(i.checked_sub(1)?)?;
        (x <= r.1).then_some(r.2)
    }
}

fn limited(code: ErrorCode, scope: LimitScope, limit: LimitKind, retry_ms: Option<u64>) -> ErrorBody {
    let mut b = ErrorBody::limited(code, scope, limit);
    b.retry_after_s = retry_ms.map(|w| w.div_ceil(1000).max(1));
    b
}

fn counters(scope: &str, tiers: &[Tier], period: &'static str, start_ms: u64, base: u32) -> Vec<CounterCheck> {
    tiers
        .iter()
        .map(|t| CounterCheck {
            scope: format!("{scope}:{}", scope_tag(t.scope)),
            key: t.key.clone(),
            period,
            start_ms,
            limit: u64::from(base) * t.factor,
        })
        .collect()
}

/// The global paced bucket of open new names (§9.4): `open_new_names_day` a day, burst a twenty-fourth.
pub fn open_names_bucket(l: &Limits) -> BucketSpec {
    let per_day = f64::from(l.open_new_names_day.max(1));
    BucketSpec {
        name: "open_new_names".into(),
        cap: (per_day / 24.0).max(1.0),
        #[allow(clippy::cast_precision_loss)]
        refill_ms: DAY_MS as f64 / per_day,
    }
}

/// `hello` of a mesh the edge has not seen, admitted open (§9.4): meshes first seen per day per network tier,
/// or, for a relay-only engine, the paced relayed-newcomer bucket and its day cap. Invited meshes skip it.
pub async fn new_mesh(edge: &Edge, net: &str, now_ms: u64) -> anyhow::Result<Result<(), ErrorBody>> {
    let l = &edge.live().limits;
    if is_relayed(net) {
        let day = CounterCheck {
            scope: "meshes:relayed".into(),
            key: "all".into(),
            period: "day",
            start_ms: day_start(now_ms),
            limit: u64::from(l.relayed_new_meshes_day),
        };
        if edge.store.counters_check(std::slice::from_ref(&day), false).await?.is_err() {
            let wait = day_start(now_ms) + DAY_MS - now_ms;
            return Ok(Err(limited(ErrorCode::Paced, LimitScope::Relayed, LimitKind::Meshes, Some(wait))));
        }
        let bucket = BucketSpec {
            name: "relayed_new_meshes".into(),
            cap: f64::from(l.relayed_burst.max(1)),
            #[allow(clippy::cast_precision_loss)]
            refill_ms: l.relayed_every.as_millis() as f64,
        };
        if let Err((_, wait)) = edge.store.buckets_take(&[bucket], now_ms).await? {
            return Ok(Err(limited(ErrorCode::Paced, LimitScope::Relayed, LimitKind::Meshes, Some(wait))));
        }
        edge.store.counters_add(&[day], 1).await?;
        return Ok(Ok(()));
    }
    let t = tiers(net, l, &edge.asn);
    let checks = counters("meshes", &t, "day", day_start(now_ms), l.new_meshes_per_net_day);
    Ok(match edge.store.counters_check(&checks, true).await? {
        Ok(()) => Ok(()),
        Err(i) => {
            let wait = day_start(now_ms) + DAY_MS - now_ms;
            Err(limited(ErrorCode::NetworkQuota, t[i].scope, LimitKind::Meshes, Some(wait)))
        }
    })
}

/// Tunnels attached per network tier (§9.4 `tunnels_per_net`), counted over the live tunnels: the refusal, if any.
pub fn tunnels_refusal(edge: &Edge, net: &str, engine: &share_proto::PublicKey) -> Option<ErrorBody> {
    let l = &edge.live().limits;
    let mine = tiers(net, l, &edge.asn);
    let nets: Vec<String> = edge
        .tunnels
        .iter()
        .filter(|t| t.engine != *engine)
        .map(|t| t.net.clone())
        .collect();
    for t in &mine {
        let n = nets.iter().filter(|n| in_tier(n, t, l, &edge.asn)).count() as u64;
        if n >= u64::from(l.tunnels_per_net) * t.factor {
            return Some(limited(ErrorCode::NetworkQuota, t.scope, LimitKind::Tunnels, None));
        }
    }
    None
}

/// `GET /v1/names/{name}` (availability lookups, LAUNCH-EDGE): per tunnel (`lookups_per_tunnel_min`), then, for a
/// direct path, per client network (IPv4 address or IPv6 /64, `lookups_per_net_min`) and per wide network (IPv4
/// /24 or IPv6 /48, `lookups_per_wide_min`). A relayed tunnel has only the tunnel limit. Refused: `429
/// rate_limited {scope, retry_after_s}` (no `limit`: share-proto has no lookup kind). In-memory GCRA, swept with
/// the other keyed limiters; sized at start. Returns the refusal, if any.
pub fn lookup(edge: &Edge, net: &str, engine: &share_proto::PublicKey) -> Option<ErrorBody> {
    use governor::clock::{Clock as _, DefaultClock};
    let refuse = |scope: LimitScope, nu: governor::NotUntil<_>| {
        let mut b = ErrorBody::new(ErrorCode::RateLimited);
        b.scope = Some(scope);
        let wait = nu.wait_time_from(DefaultClock::default().now());
        b.retry_after_s = Some(u64::try_from(wait.as_millis().div_ceil(1000)).unwrap_or(u64::MAX).max(1));
        b
    };
    if let Err(nu) = edge.lookup_tunnel.check_key(engine) {
        return Some(refuse(LimitScope::Tunnel, nu));
    }
    if let Ok(ip) = net.parse::<IpAddr>() {
        let ip = crate::state::net_key(ip);
        let wide = match ip {
            IpAddr::V4(v4) => {
                let o = v4.octets();
                IpAddr::V4(Ipv4Addr::new(o[0], o[1], o[2], 0))
            }
            IpAddr::V6(v6) => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], 0, 0, 0, 0, 0))
            }
        };
        if let Err(nu) = edge.lookup_net.check_key(&ip) {
            return Some(refuse(LimitScope::Net, nu));
        }
        if let Err(nu) = edge.lookup_wide.check_key(&wide) {
            return Some(refuse(LimitScope::NetWide, nu));
        }
    }
    None
}

/// What a granted-before-claim check took, to give back when the claim is refused.
#[derive(Debug, Default)]
pub struct Taken {
    open_name: Option<BucketSpec>,
}

impl Taken {
    /// The claim was not granted: the global open-names token goes back.
    pub async fn refund(self, edge: &Edge, now_ms: u64) -> anyhow::Result<()> {
        if let Some(b) = self.open_name {
            edge.store.bucket_refund(&b, now_ms).await?;
        }
        Ok(())
    }
}

/// Before a claim or allocation (§9.4, §6.5): `edge_full` (names_max reached and nothing evictable, or
/// `queue_max` queued); for open meshes the per-network claim rate (`creates_per_net_hour`, burst
/// `creates_burst`), reserved names held per network, then the global paced open-names bucket.
pub async fn new_name(
    edge: &Edge,
    invited: bool,
    net: &str,
    kind: NameKind,
    now_ms: u64,
) -> anyhow::Result<Result<Taken, ErrorBody>> {
    let l = &edge.live().limits;
    if edge.full() {
        return Ok(Err(ErrorBody::edge_full(LimitKind::NamesMax)));
    }
    let domain = edge.cfg.domain.as_str();
    if edge.store.slot_jobs(domain).await?.len() as u64 >= u64::from(edge.live().queue_max) {
        return Ok(Err(ErrorBody::edge_full(LimitKind::QueueMax)));
    }
    if invited {
        return Ok(Ok(Taken::default()));
    }
    let t = tiers(net, l, &edge.asn);
    if kind == NameKind::Reserved {
        let held = edge.store.open_reserved_nets(domain).await?;
        for tier in &t {
            let n = held.iter().filter(|h| in_tier(h, tier, l, &edge.asn)).count() as u64;
            if n >= u64::from(l.reserved_per_net) * tier.factor {
                return Ok(Err(limited(ErrorCode::NetworkQuota, tier.scope, LimitKind::Reserved, None)));
            }
        }
    }
    let mut specs: Vec<BucketSpec> = t
        .iter()
        .map(|tier| {
            #[allow(clippy::cast_precision_loss)]
            let f = tier.factor as f64;
            BucketSpec {
                name: format!("claims:{}:{}", scope_tag(tier.scope), tier.key),
                cap: f64::from(l.creates_burst.max(1)) * f,
                #[allow(clippy::cast_precision_loss)]
                refill_ms: HOUR_MS as f64 / (f64::from(l.creates_per_net_hour.max(1)) * f),
            }
        })
        .collect();
    let global = open_names_bucket(l);
    specs.push(global.clone());
    Ok(match edge.store.buckets_take(&specs, now_ms).await? {
        Ok(()) => Ok(Taken {
            open_name: Some(global),
        }),
        Err((i, wait)) if i == t.len() => Err(limited(ErrorCode::Paced, LimitScope::Global, LimitKind::Names, Some(wait))),
        Err((i, wait)) => Err(limited(ErrorCode::NetworkQuota, t[i].scope, LimitKind::Claims, Some(wait))),
    })
}

fn cert_counters(edge: &Edge, mesh: &str, invited: bool, net: &str, now_ms: u64) -> Vec<CounterCheck> {
    let l = &edge.live().limits;
    let mut v = vec![CounterCheck {
        scope: "certs:mesh".into(),
        key: mesh.to_string(),
        period: "week",
        start_ms: week_start(now_ms),
        limit: u64::from(l.certs_per_mesh_week),
    }];
    if !invited {
        // Invited meshes leave every per-network onboarding count (F-m9).
        v.extend(counters("certs", &tiers(net, l, &edge.asn), "week", week_start(now_ms), l.certs_per_net_week));
    }
    v
}

/// A first CSR that will wait for a slot (§6.5, §9.4): `queue_max`, the mesh's new-name certificates this
/// week, and for open meshes the network's certificates this week and its names already queued.
pub async fn cert_request(
    edge: &Edge,
    mesh: &str,
    invited: bool,
    net: &str,
    now_ms: u64,
) -> anyhow::Result<Result<(), ErrorBody>> {
    let l = &edge.live().limits;
    let jobs = edge.store.slot_jobs(&edge.cfg.domain).await?;
    if jobs.len() as u64 >= u64::from(edge.live().queue_max) {
        return Ok(Err(ErrorBody::edge_full(LimitKind::QueueMax)));
    }
    let checks = cert_counters(edge, mesh, invited, net, now_ms);
    if let Err(i) = edge.store.counters_check(&checks, false).await? {
        let wait = week_start(now_ms) + WEEK_MS - now_ms;
        return Ok(Err(if i == 0 {
            limited(ErrorCode::Quota, LimitScope::Mesh, LimitKind::Certs, Some(wait))
        } else {
            let t = tiers(net, l, &edge.asn);
            limited(ErrorCode::NetworkQuota, t[i - 1].scope, LimitKind::Certs, Some(wait))
        }));
    }
    if !invited {
        for tier in tiers(net, l, &edge.asn) {
            let n = jobs
                .iter()
                .filter(|j| j.class == "open" && in_tier(&j.net, &tier, l, &edge.asn))
                .count() as u64;
            if n >= u64::from(l.queued_per_net) * tier.factor {
                return Ok(Err(limited(ErrorCode::NetworkQuota, tier.scope, LimitKind::Queued, None)));
            }
        }
    }
    Ok(Ok(()))
}

/// A job got a slot: count it against its mesh's and (open meshes) its network's weekly certificates.
pub async fn count_slot(edge: &Edge, mesh: &str, invited: bool, net: &str, now_ms: u64) -> anyhow::Result<()> {
    let c = cert_counters(edge, mesh, invited, net, now_ms);
    edge.store.counters_add(&c, 1).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_cover_v4_v6_and_asn() {
        let l = Limits::default();
        let asn = AsnDb::parse("203.0.113.0\t203.0.113.255\t64500\tZZ\tTEST\n2001:db8::/32 64501\n10.0.0.0 10.255.255.255 0 None\n");
        let t = tiers("203.0.113.7", &l, &asn);
        let keys: Vec<_> = t.iter().map(|t| (t.scope, t.key.as_str(), t.factor)).collect();
        assert_eq!(
            keys,
            [
                (LimitScope::Net, "203.0.113.7", 1),
                (LimitScope::NetWide, "203.0.113.0/24", 4),
                (LimitScope::Asn, "as64500", 64),
            ]
        );
        let t = tiers("2001:db8:aa:bb::", &l, &asn);
        let keys: Vec<_> = t.iter().map(|t| (t.scope, t.key.as_str(), t.factor)).collect();
        assert_eq!(
            keys,
            [
                (LimitScope::Net, "2001:db8:aa:bb::", 1),
                (LimitScope::NetWide, "2001:db8:aa::/48", 4),
                (LimitScope::Net32, "2001:db8::/32", 16),
                (LimitScope::Asn, "as64501", 64),
            ]
        );
        // ASN 0 is "not routed": no tier. A relay has no tiers at all.
        assert_eq!(tiers("10.1.2.3", &l, &asn).len(), 2);
        assert!(tiers("relay:https://r.example/", &l, &asn).is_empty());
        assert!(AsnDb::default().lookup("203.0.113.7".parse().unwrap()).is_none());
    }

    #[test]
    fn month_bounds_are_utc_months() {
        // 2026-02-10T12:00:00Z → February 2026, 28 days.
        let (start, len) = month_bounds(1_770_724_800_000);
        assert_eq!(start, 1_769_904_000_000);
        assert_eq!(len, 28 * DAY_MS);
    }
}
