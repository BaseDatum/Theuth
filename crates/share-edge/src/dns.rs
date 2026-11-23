//! DNS records of Path Z (210 §6.0, §6.4, D17; appendix §6.1): the thin `DnsZone` trait over one provider, the
//! zone-update Porkbun impl, and the checked helpers every caller goes through (read-back after each write and
//! delete, provider panics contained).

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::panic::AssertUnwindSafe;

use anyhow::{Context as _, anyhow, bail};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use serde::Deserialize;

use crate::config::SecretSource;

/// The record types the edge writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rtype {
    /// `edge.D` only (operator-written today).
    A,
    /// `<n>.D` and `*.<n>.D` → `edge.D` (D17).
    Cname,
    /// `_acme-challenge.<n>.D`, one at a time (§6.1).
    Txt,
}

impl Rtype {
    /// The `dns_records.type` text.
    pub fn as_str(self) -> &'static str {
        match self {
            Rtype::A => "A",
            Rtype::Cname => "CNAME",
            Rtype::Txt => "TXT",
        }
    }

    /// Inverse of `as_str`.
    pub fn parse(s: &str) -> Option<Rtype> {
        match s {
            "A" => Some(Rtype::A),
            "CNAME" => Some(Rtype::Cname),
            "TXT" => Some(Rtype::Txt),
            _ => None,
        }
    }
}

impl fmt::Display for Rtype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One DNS zone. Hosts are relative to the zone (`sky`, `*.sky`, `_acme-challenge.sky`). Callers use the checked
/// helpers below, never these directly.
pub trait DnsZone: Send + Sync + fmt::Debug {
    /// The single record of `rtype` at `host`; an error when the host holds more than one.
    fn get(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<Option<String>>>;
    /// Add a record.
    fn create(&self, rtype: Rtype, host: String, value: String) -> BoxFuture<'_, anyhow::Result<()>>;
    /// Delete every record of `rtype` at `host`.
    fn delete_all(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<()>>;
}

/// Run one provider call; a panic inside it is an error of this attempt, never a worker crash (§6.1).
pub async fn contain<T>(f: BoxFuture<'_, anyhow::Result<T>>) -> anyhow::Result<T> {
    match AssertUnwindSafe(f).catch_unwind().await {
        Ok(r) => r,
        Err(p) => Err(anyhow!("DNS provider panicked: {}", panic_text(&*p))),
    }
}

/// The message of a panic payload.
pub fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic".into())
}

fn same(rtype: Rtype, a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_matches('"').trim_end_matches('.').to_string();
    match rtype {
        Rtype::Txt => norm(a) == norm(b),
        _ => norm(a).eq_ignore_ascii_case(&norm(b)),
    }
}

/// Make `host` hold exactly `value`, then read it back.
pub async fn ensure(zone: &dyn DnsZone, rtype: Rtype, host: &str, value: &str) -> anyhow::Result<()> {
    match contain(zone.get(rtype, host.into())).await {
        Ok(Some(v)) if same(rtype, &v, value) => return Ok(()),
        Ok(None) => {}
        // A different value, or several (zone-update refuses to read those): clear and write.
        Ok(Some(_)) | Err(_) => remove(zone, rtype, host).await?,
    }
    contain(zone.create(rtype, host.into(), value.into())).await?;
    match contain(zone.get(rtype, host.into())).await? {
        Some(v) if same(rtype, &v, value) => Ok(()),
        other => bail!("read-back of {rtype} {host}: expected {value:?}, found {other:?}"),
    }
}

/// Delete every `rtype` record at `host`, then read back that none is left (Porkbun deletes are unchecked).
pub async fn remove(zone: &dyn DnsZone, rtype: Rtype, host: &str) -> anyhow::Result<()> {
    contain(zone.delete_all(rtype, host.into())).await?;
    match contain(zone.get(rtype, host.into())).await? {
        None => Ok(()),
        Some(v) => bail!("read-back of {rtype} {host} after delete: still {v:?}"),
    }
}

/// `fqdn` relative to `zone` (`sky.tun.zip` in `tun.zip` → `sky`).
pub fn relative(fqdn: &str, zone: &str) -> anyhow::Result<String> {
    let fqdn = fqdn.trim_end_matches('.');
    fqdn.strip_suffix(zone)
        .and_then(|h| h.strip_suffix('.'))
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{fqdn} is not below zone {zone}"))
}

/// The two D17 hosts of a name, relative to `zone`, in write order: `*.<n>` first, then `<n>` (delete in reverse).
/// While `<n>` exists without `*.<n>`, `x.<n>.D` is NXDOMAIN, cacheable for the zone's SOA minimum (1800 s at
/// Porkbun); before `<n>` exists the zone wildcard `*.D` still answers for it (E2E1 finding 5).
pub fn name_hosts(name: &str, domain: &str, zone: &str) -> anyhow::Result<[String; 2]> {
    let bare = relative(&format!("{name}.{domain}"), zone)?;
    Ok([format!("*.{bare}"), bare])
}

/// The challenge TXT host of an ACME identifier (`*.sky.tun.zip` and `sky.tun.zip` → `_acme-challenge.sky`).
pub fn challenge_host(ident: &str, zone: &str) -> anyhow::Result<String> {
    let base = ident.strip_prefix("*.").unwrap_or(ident);
    relative(&format!("_acme-challenge.{base}"), zone)
}

/// The Porkbun credential (§8.3). zone-update's `porkbun::Auth` names the fields `key`/`secret`; Porkbun's own
/// API names them `apikey`/`secretapikey`; both spellings are accepted.
#[derive(Deserialize)]
struct PorkbunCredential {
    #[serde(alias = "apikey")]
    key: String,
    #[serde(alias = "secretapikey")]
    secret: String,
}

/// zone-update's concrete Porkbun client (not the boxed trait object: its generic `create_record` is
/// `Self: Sized`, so a CNAME needs the concrete type). The credential is re-read on every call, so a rotated
/// key needs no restart; each call runs in `spawn_blocking` (zone-update is blocking `ureq`).
pub struct PorkbunZone {
    zone: String,
    credential: SecretSource,
}

impl fmt::Debug for PorkbunZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PorkbunZone").field("zone", &self.zone).finish_non_exhaustive()
    }
}

impl PorkbunZone {
    /// A client for `zone` with the credential from `credential`. Fails early on an unreadable credential.
    pub fn new(zone: &str, credential: SecretSource) -> anyhow::Result<PorkbunZone> {
        let z = PorkbunZone {
            zone: zone.to_string(),
            credential,
        };
        z.client().context("dns credential")?;
        Ok(z)
    }

    fn client(&self) -> anyhow::Result<zone_update::porkbun::Porkbun> {
        let c: PorkbunCredential = serde_json::from_str(&self.credential.read()?)
            .context("dns credential: expected JSON {\"key\": …, \"secret\": …}")?;
        Ok(zone_update::porkbun::Porkbun::new(
            zone_update::Config {
                domain: self.zone.clone(),
                dry_run: false,
            },
            zone_update::porkbun::Auth {
                key: c.key,
                secret: c.secret,
            },
        ))
    }

    fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(zone_update::porkbun::Porkbun) -> zone_update::errors::Result<T> + Send + 'static,
    ) -> BoxFuture<'_, anyhow::Result<T>> {
        async move {
            let client = self.client()?;
            match tokio::task::spawn_blocking(move || f(client)).await {
                Ok(r) => r.map_err(|e| anyhow!("porkbun: {e}")),
                Err(e) if e.is_panic() => {
                    Err(anyhow!("porkbun client panicked: {}", panic_text(&*e.into_panic())))
                }
                Err(e) => Err(anyhow!("porkbun call: {e}")),
            }
        }
        .boxed()
    }
}

fn record_type(t: Rtype) -> zone_update::RecordType {
    match t {
        Rtype::A => zone_update::RecordType::A,
        Rtype::Cname => zone_update::RecordType::CNAME,
        Rtype::Txt => zone_update::RecordType::TXT,
    }
}

impl DnsZone for PorkbunZone {
    fn get(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<Option<String>>> {
        use zone_update::DnsProvider as _;
        self.call(move |c| c.get_record::<String>(record_type(rtype), &host))
    }

    fn create(&self, rtype: Rtype, host: String, value: String) -> BoxFuture<'_, anyhow::Result<()>> {
        use zone_update::DnsProvider as _;
        // TODO(210-M1): zone-update sends TTL 300; P0-1 records whether Porkbun accepts it (D17 plans 600).
        self.call(move |c| c.create_record(record_type(rtype), &host, &value))
    }

    fn delete_all(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<()>> {
        use zone_update::DnsProvider as _;
        self.call(move |c| c.delete_all_records(record_type(rtype), &host))
    }
}

/// Where a challenge TXT must be visible before the CA is told to look (§6.1): the zone's authoritative
/// servers. FIX-EDGE m9 replaces the fixed `acme.propagation` sleep (TODO(210-M1)) with this poll.
pub trait TxtProbe: Send + Sync + fmt::Debug {
    /// Return once every authoritative server of `zone` answers `value` for the absolute `fqdn`; an error when
    /// `deadline` passes first.
    fn wait<'a>(
        &'a self,
        zone: &'a str,
        fqdn: &'a str,
        value: &'a str,
        deadline: std::time::Duration,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// An NS name with all its A and AAAA addresses.
pub type NameServer = (String, Vec<IpAddr>);

/// The DNS lookups [`AuthoritativeTxt`] needs; tests inject a fake.
pub trait NsResolver: Send + Sync + fmt::Debug {
    /// The authoritative servers of `zone`: each NS name with all its A and AAAA addresses.
    fn servers<'a>(&'a self, zone: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<NameServer>>>;
    /// Whether this host has a global IPv6 address with a route towards `probe` (no packet is sent).
    fn global_v6(&self, probe: Ipv6Addr) -> bool;
    /// The TXT strings `server` answers (non-recursively) for `fqdn`; empty for NXDOMAIN / no record. An error
    /// means the server did not answer (timeout, unreachable).
    fn txt<'a>(&'a self, server: IpAddr, fqdn: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<String>>>;
}

/// [`TxtProbe`] over an [`NsResolver`]: every NS is asked over its addresses in preference order (IPv4 first
/// unless the host has global IPv6; the other family is the fallback), the first address that answers serves
/// that NS from then on. Polled every `interval` until each NS that ever answered has the value (E2E1 finding
/// 3: the pod has no IPv6 route, Porkbun's NS answer over IPv4).
#[derive(Debug)]
pub struct AuthoritativeTxt {
    resolver: std::sync::Arc<dyn NsResolver>,
    interval: std::time::Duration,
}

impl Default for AuthoritativeTxt {
    fn default() -> AuthoritativeTxt {
        AuthoritativeTxt::new(std::sync::Arc::new(HickoryNs), std::time::Duration::from_secs(2))
    }
}

impl AuthoritativeTxt {
    /// A probe over `resolver`, polling every `interval`.
    pub fn new(resolver: std::sync::Arc<dyn NsResolver>, interval: std::time::Duration) -> AuthoritativeTxt {
        AuthoritativeTxt { resolver, interval }
    }
}

/// One NS during a wait.
struct Ns {
    name: String,
    /// Addresses in the order they are tried; the one that last answered moves to the front.
    addrs: Vec<IpAddr>,
    answered: bool,
    seen: bool,
}

/// Ask `ns` over its addresses in order; the first that answers decides. `None` when none answered.
async fn ask(r: &dyn NsResolver, ns: &Ns, fqdn: &str, value: &str) -> Option<(IpAddr, bool)> {
    for &ip in &ns.addrs {
        match r.txt(ip, fqdn).await {
            Ok(vals) => return Some((ip, vals.iter().any(|v| same(Rtype::Txt, v, value)))),
            Err(e) => tracing::debug!(ns = %ns.name, addr = %ip, %fqdn, "TXT probe: no answer: {e:#}"),
        }
    }
    None
}

impl TxtProbe for AuthoritativeTxt {
    fn wait<'a>(
        &'a self,
        zone: &'a str,
        fqdn: &'a str,
        value: &'a str,
        deadline: std::time::Duration,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        async move {
            let r = &*self.resolver;
            let end = tokio::time::Instant::now() + deadline;
            let servers = r.servers(zone).await?;
            let v6 = servers
                .iter()
                .flat_map(|(_, a)| a)
                .find_map(|a| match a {
                    IpAddr::V6(a) => Some(*a),
                    IpAddr::V4(_) => None,
                })
                .is_some_and(|probe| r.global_v6(probe));
            let mut nss: Vec<Ns> = servers
                .into_iter()
                .filter(|(_, a)| !a.is_empty())
                .map(|(name, mut addrs)| {
                    // Stable: v4 before v6 without global IPv6, v6 before v4 with it.
                    addrs.sort_by_key(|a| a.is_ipv6() != v6);
                    Ns { name, addrs, answered: false, seen: false }
                })
                .collect();
            if nss.is_empty() {
                bail!("no authoritative server found for {zone}");
            }
            tracing::debug!(%fqdn, global_v6 = v6, "TXT probe over {} NS", nss.len());
            loop {
                let pending: Vec<usize> = (0..nss.len()).filter(|&i| !nss[i].seen).collect();
                let answers =
                    futures::future::join_all(pending.iter().map(|&i| ask(r, &nss[i], fqdn, value))).await;
                for (i, a) in pending.into_iter().zip(answers) {
                    let Some((ip, has)) = a else { continue };
                    let ns = &mut nss[i];
                    ns.answered = true;
                    ns.seen = has;
                    if let Some(p) = ns.addrs.iter().position(|a| *a == ip) {
                        ns.addrs[..=p].rotate_right(1);
                    }
                    if has {
                        tracing::info!(%fqdn, ns = %ns.name, addr = %ip, "challenge TXT visible");
                    }
                }
                let missing: Vec<&str> =
                    nss.iter().filter(|n| n.answered && !n.seen).map(|n| n.name.as_str()).collect();
                let silent: Vec<&str> = nss.iter().filter(|n| !n.answered).map(|n| n.name.as_str()).collect();
                if missing.is_empty() && nss.iter().any(|n| n.seen) {
                    if !silent.is_empty() {
                        tracing::warn!(%fqdn, "TXT probe: no answer from {silent:?}, not waited for");
                    }
                    return Ok(());
                }
                if tokio::time::Instant::now() >= end {
                    bail!("{fqdn} TXT not yet on {missing:?} (no answer from {silent:?}) after {deadline:?}");
                }
                tokio::time::sleep(self.interval).await;
            }
        }
        .boxed()
    }
}

/// [`NsResolver`] over hickory: NS and A/AAAA from the system resolver, one uncached, non-recursive resolver per
/// server address (3 s timeout, one attempt).
#[derive(Debug, Default)]
pub struct HickoryNs;

impl NsResolver for HickoryNs {
    fn servers<'a>(&'a self, zone: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<NameServer>>> {
        async move {
            use hickory_resolver::TokioResolver;
            use hickory_resolver::proto::rr::{RData, RecordType};

            let system = TokioResolver::builder_tokio()?.build()?;
            let ns = system.lookup(format!("{zone}."), RecordType::NS).await?;
            let mut out = Vec::new();
            for r in ns.answers() {
                let RData::NS(n) = &r.data else { continue };
                let name = n.0.to_string();
                let mut addrs = Vec::new();
                for t in [RecordType::A, RecordType::AAAA] {
                    match system.lookup(name.as_str(), t).await {
                        Ok(l) => addrs.extend(l.answers().iter().filter_map(|rec| match &rec.data {
                            RData::A(a) => Some(IpAddr::V4(a.0)),
                            RData::AAAA(a) => Some(IpAddr::V6(a.0)),
                            _ => None,
                        })),
                        Err(e) => tracing::debug!(ns = %name, "{t} lookup: {e}"),
                    }
                }
                out.push((name, addrs));
            }
            Ok(out)
        }
        .boxed()
    }

    fn global_v6(&self, probe: Ipv6Addr) -> bool {
        // A UDP connect only picks a route and source address.
        let local = std::net::UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0))
            .and_then(|s| s.connect((probe, 53)).and_then(|()| s.local_addr()));
        match local.map(|a| a.ip()) {
            Ok(IpAddr::V6(a)) => is_global_v6(a),
            _ => false,
        }
    }

    fn txt<'a>(&'a self, server: IpAddr, fqdn: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<String>>> {
        async move {
            use hickory_resolver::TokioResolver;
            use hickory_resolver::config::{NameServerConfig, ResolverConfig};
            use hickory_resolver::net::runtime::TokioRuntimeProvider;
            use hickory_resolver::proto::rr::{RData, RecordType};

            let mut cfg = ResolverConfig::default();
            cfg.add_name_server(NameServerConfig::udp_and_tcp(server));
            let mut b = TokioResolver::builder_with_config(cfg, TokioRuntimeProvider::default());
            let o = b.options_mut();
            o.cache_size = 0;
            o.recursion_desired = false;
            o.timeout = std::time::Duration::from_secs(3);
            o.attempts = 1;
            match b.build()?.lookup(fqdn, RecordType::TXT).await {
                Ok(l) => Ok(l
                    .answers()
                    .iter()
                    .filter_map(|rec| match &rec.data {
                        RData::TXT(t) => Some(t.txt_data.iter().map(|s| String::from_utf8_lossy(s)).collect()),
                        _ => None,
                    })
                    .collect()),
                Err(e) if e.is_no_records_found() => Ok(Vec::new()),
                Err(e) => Err(e.into()),
            }
        }
        .boxed()
    }
}

/// Not loopback, unspecified, link-local (`fe80::/10`), unique-local (`fc00::/7`) or v4-mapped.
fn is_global_v6(a: Ipv6Addr) -> bool {
    let s = a.segments()[0];
    !(a.is_loopback()
        || a.is_unspecified()
        || (s & 0xffc0) == 0xfe80
        || (s & 0xfe00) == 0xfc00
        || a.to_ipv4_mapped().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_relative_to_the_zone() {
        // Write order: wildcard first (E2E1 finding 5).
        assert_eq!(
            name_hosts("sky", "tun.zip", "tun.zip").unwrap(),
            ["*.sky".to_string(), "sky".to_string()]
        );
        assert_eq!(
            name_hosts("sky", "share.example.com", "example.com").unwrap(),
            ["*.sky.share".to_string(), "sky.share".to_string()]
        );
        assert_eq!(
            challenge_host("*.sky.tun.zip", "tun.zip").unwrap(),
            "_acme-challenge.sky"
        );
        assert_eq!(challenge_host("sky.tun.zip", "tun.zip").unwrap(), "_acme-challenge.sky");
        assert!(relative("sky.other.zip", "tun.zip").is_err());
        assert!(relative("tun.zip", "tun.zip").is_err());
    }

    #[test]
    fn credential_accepts_both_spellings() {
        let a: PorkbunCredential = serde_json::from_str(r#"{"key":"pk1_a","secret":"sk1_b"}"#).unwrap();
        let b: PorkbunCredential =
            serde_json::from_str(r#"{"apikey":"pk1_a","secretapikey":"sk1_b"}"#).unwrap();
        assert_eq!((a.key, a.secret), (b.key, b.secret));
    }

    /// One scripted answer; `Err(())` is a timeout.
    type Answer = Result<Vec<String>, ()>;

    /// Scripted authoritative servers: each address answers its queue in turn (the last answer repeats);
    /// `Err(())` is a timeout.
    #[derive(Debug, Default)]
    struct FakeNs {
        servers: Vec<NameServer>,
        v6: bool,
        script: std::sync::Mutex<std::collections::HashMap<IpAddr, Vec<Answer>>>,
        queries: std::sync::Mutex<Vec<IpAddr>>,
    }

    impl FakeNs {
        fn new(v6: bool, servers: &[(&str, &[&str])]) -> FakeNs {
            FakeNs {
                servers: servers
                    .iter()
                    .map(|(n, a)| (n.to_string(), a.iter().map(|a| a.parse().unwrap()).collect()))
                    .collect(),
                v6,
                ..FakeNs::default()
            }
        }

        fn answers(self, ip: &str, answers: Vec<Answer>) -> FakeNs {
            self.script.lock().unwrap().insert(ip.parse().unwrap(), answers);
            self
        }

        fn queried(&self) -> Vec<String> {
            self.queries.lock().unwrap().iter().map(ToString::to_string).collect()
        }
    }

    impl NsResolver for FakeNs {
        fn servers<'a>(&'a self, _zone: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<NameServer>>> {
            async move { Ok(self.servers.clone()) }.boxed()
        }

        fn global_v6(&self, _probe: Ipv6Addr) -> bool {
            self.v6
        }

        fn txt<'a>(&'a self, server: IpAddr, _fqdn: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<String>>> {
            self.queries.lock().unwrap().push(server);
            let mut s = self.script.lock().unwrap();
            let q = s.entry(server).or_insert_with(|| vec![Err(())]);
            let a = if q.len() > 1 { q.remove(0) } else { q[0].clone() };
            async move { a.map_err(|()| anyhow!("timeout")) }.boxed()
        }
    }

    const V: &str = "tok";
    const FQDN: &str = "_acme-challenge.sky.tun.zip.";

    fn has() -> Answer {
        Ok(vec![format!("\"{V}\"")])
    }

    async fn probe(ns: &std::sync::Arc<FakeNs>, deadline: u64) -> anyhow::Result<()> {
        AuthoritativeTxt::new(ns.clone(), std::time::Duration::ZERO)
            .wait("tun.zip", FQDN, V, std::time::Duration::from_secs(deadline))
            .await
    }

    #[tokio::test]
    async fn without_global_v6_each_ns_is_asked_over_ipv4() {
        // E2E1 finding 3: the pod has no IPv6 route; the AAAA addresses would time out.
        let ns = std::sync::Arc::new(
            FakeNs::new(false, &[("a.ns.", &["2001:db8::1", "192.0.2.1"]), ("b.ns.", &["192.0.2.2", "2001:db8::2"])])
                .answers("192.0.2.1", vec![has()])
                .answers("192.0.2.2", vec![has()]),
        );
        probe(&ns, 10).await.unwrap();
        assert_eq!(ns.queried(), ["192.0.2.1", "192.0.2.2"]);
    }

    #[tokio::test]
    async fn the_other_family_answers_when_the_preferred_one_does_not() {
        // Global v6: AAAA first; it times out, the A answers.
        let ns = std::sync::Arc::new(
            FakeNs::new(true, &[("a.ns.", &["192.0.2.1", "2001:db8::1"])]).answers("192.0.2.1", vec![has()]),
        );
        probe(&ns, 10).await.unwrap();
        assert_eq!(ns.queried(), ["2001:db8::1", "192.0.2.1"]);
        // No v6 at all and an NS with only AAAA: still tried.
        let ns = std::sync::Arc::new(
            FakeNs::new(false, &[("a.ns.", &["2001:db8::1"])]).answers("2001:db8::1", vec![has()]),
        );
        probe(&ns, 10).await.unwrap();
    }

    #[tokio::test]
    async fn waits_for_every_answering_ns_and_skips_silent_ones() {
        let ns = std::sync::Arc::new(
            FakeNs::new(false, &[("a.ns.", &["192.0.2.1"]), ("b.ns.", &["192.0.2.2"]), ("c.ns.", &["192.0.2.3"])])
                .answers("192.0.2.1", vec![has()])
                .answers("192.0.2.2", vec![Ok(vec![]), Ok(vec!["other".into()]), has()]),
        );
        probe(&ns, 10).await.unwrap();
        // a once (seen in round 1), b until it has the value, c (silent) every round but not waited for.
        assert_eq!(
            ns.queried(),
            ["192.0.2.1", "192.0.2.2", "192.0.2.3", "192.0.2.2", "192.0.2.3", "192.0.2.2", "192.0.2.3"]
        );
    }

    #[tokio::test]
    async fn an_ns_that_answers_without_the_value_fails_at_the_deadline() {
        let ns = std::sync::Arc::new(
            FakeNs::new(false, &[("a.ns.", &["192.0.2.1"]), ("b.ns.", &["192.0.2.2"])])
                .answers("192.0.2.1", vec![has()])
                .answers("192.0.2.2", vec![Ok(vec![])]),
        );
        let e = probe(&ns, 0).await.unwrap_err().to_string();
        assert!(e.contains("b.ns."), "{e}");
        // Nobody answers: an error too, never a silent pass.
        let ns = std::sync::Arc::new(FakeNs::new(false, &[("a.ns.", &["192.0.2.1"])]));
        let e = probe(&ns, 0).await.unwrap_err().to_string();
        assert!(e.contains("no answer from [\"a.ns.\"]"), "{e}");
    }

    #[test]
    fn global_v6_excludes_local_scopes() {
        for a in ["::1", "::", "fe80::1", "fd00::1", "::ffff:192.0.2.1"] {
            assert!(!is_global_v6(a.parse().unwrap()), "{a}");
        }
        assert!(is_global_v6("2a01:4f8::1".parse().unwrap()));
    }
}
