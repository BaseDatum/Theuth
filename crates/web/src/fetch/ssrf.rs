//! SSRF guard for `read_url` (design doc 16 §5): scheme / credential checks,
//! a hostname denylist, IP-range classification, and DNS resolve-and-check.
//!
//! `private` (16 §5 as amended 2026-11-11) admits this machine's local
//! network: loopback, RFC 1918, CGNAT, link-local, IPv6 ULA / link-local and
//! the internal hostnames. Cloud metadata endpoints, unspecified, broadcast,
//! multicast and the reserved / documentation / tunnel ranges stay refused
//! either way.
//!
//! [`check_url`] is the whole gate for callers that hand the URL to someone
//! else (the rendered-fetch path). The fetcher itself runs [`check_static`]
//! on every redirect hop and plugs [`GuardedResolver`] into its `reqwest`
//! client, so the addresses it connects to are exactly the ones that passed
//! [`is_blocked_ip`] - no resolve-then-connect window for DNS rebinding.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::{Host, Url};

use super::FetchError;

/// Parse, statically check, and resolve-and-check `raw`. Public so the render
/// path can vet a URL before sending it to a provider.
///
/// # Errors
/// [`FetchError::Rejected`] when the URL is malformed, uses a scheme other
/// than http(s), carries credentials, names a denylisted host, or resolves to
/// an address [`is_blocked_ip`] refuses under `private`.
pub async fn check_url(raw: &str, private: bool) -> Result<Url, FetchError> {
    let url = parse_absolute(raw)?;
    check_static(&url, &[], private)?;
    if let Some(Host::Domain(domain)) = url.host() {
        resolve_checked(domain, private).await?;
    }
    Ok(url)
}

/// Parse an absolute URL (trimmed); `Rejected` when it is relative or invalid.
pub(super) fn parse_absolute(raw: &str) -> Result<Url, FetchError> {
    let url =
        Url::parse(raw.trim()).map_err(|e| FetchError::Rejected(format!("invalid URL: {e}")))?;
    if url.cannot_be_a_base() {
        return Err(FetchError::Rejected("URL must be absolute http(s)".into()));
    }
    Ok(url)
}

/// Scheme, userinfo, hostname denylist, and IP-literal checks (no DNS).
///
/// `allow` is the test-only socket allowlist: an IP literal that would be
/// blocked passes when its exact `ip:port` is listed (loopback wiremock).
pub(super) fn check_static(
    url: &Url,
    allow: &[SocketAddr],
    private: bool,
) -> Result<(), FetchError> {
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(FetchError::Rejected(format!(
            "scheme `{scheme}` not allowed (http/https only)"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::Rejected(
            "credentials in URL are not allowed".into(),
        ));
    }
    let host = url
        .host()
        .ok_or_else(|| FetchError::Rejected("URL missing host".into()))?;
    let port = url.port_or_known_default().unwrap_or(0);
    match host {
        Host::Domain(domain) if is_blocked_hostname(domain, private) => Err(FetchError::Rejected(
            format!("hostname `{domain}` is not allowed"),
        )),
        Host::Domain(_) => Ok(()),
        Host::Ipv4(ip) => assert_ip_allowed(IpAddr::V4(ip), port, allow, private),
        Host::Ipv6(ip) => assert_ip_allowed(IpAddr::V6(ip), port, allow, private),
    }
}

fn assert_ip_allowed(
    ip: IpAddr,
    port: u16,
    allow: &[SocketAddr],
    private: bool,
) -> Result<(), FetchError> {
    if !is_blocked_ip(ip, private) || allow.contains(&SocketAddr::new(ip, port)) {
        return Ok(());
    }
    Err(FetchError::Rejected(format!(
        "IP address `{ip}` is not allowed"
    )))
}

/// Resolve `host` and require **every** returned address to pass [`is_blocked_ip`].
async fn resolve_checked(host: &str, private: bool) -> Result<Vec<SocketAddr>, FetchError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|e| FetchError::Rejected(format!("DNS resolve failed for `{host}`: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(FetchError::Rejected(format!(
            "DNS returned no addresses for `{host}`"
        )));
    }
    if let Some(bad) = addrs.iter().find(|a| is_blocked_ip(a.ip(), private)) {
        return Err(FetchError::Rejected(format!(
            "hostname `{host}` resolves to blocked address `{}`",
            bad.ip()
        )));
    }
    Ok(addrs)
}

/// `reqwest` DNS resolver that refuses what [`is_blocked_ip`] refuses under
/// `private`, so the client can only ever connect to what the guard cleared.
/// IP literals never reach a resolver; [`check_static`] covers those.
pub(super) struct GuardedResolver {
    pub(super) private: bool,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let private = self.private;
        Box::pin(async move {
            let addrs = resolve_checked(name.as_str(), private).await?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Internal-network suffixes (also blocked as the bare label).
const INTERNAL_SUFFIXES: &[&str] = &[
    ".localhost",
    ".local",
    ".internal",
    ".lan",
    ".home.arpa",
    ".intranet",
    ".corp",
];

/// Cloud metadata and cluster-API aliases: refused even under `private`.
const METADATA_HOSTS: &[&str] = &[
    "metadata.google.internal",
    "metadata",
    "kubernetes.default",
    "kubernetes.default.svc",
    "kubernetes.default.svc.cluster.local",
];

/// Whether a hostname is refused without DNS: the cloud metadata aliases
/// always; unless `private`, also `localhost`, `*.localhost`, `*.local`
/// (mDNS), `*.internal`, `*.lan`, `*.home.arpa`, `*.intranet`, `*.corp` (and
/// each as a bare label).
#[must_use]
pub fn is_blocked_hostname(host: &str, private: bool) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h.is_empty()
        || METADATA_HOSTS.contains(&h.as_str())
        || (!private
            && (h == "localhost"
                || INTERNAL_SUFFIXES
                    .iter()
                    .any(|s| h.ends_with(s) || h == s[1..])))
}

/// Whether an IP must not be contacted.
///
/// Always: cloud metadata (`169.254.169.254`, `169.254.170.2`,
/// `100.100.100.200`, `fd00:ec2::254`), unspecified and `0.0.0.0/8`,
/// broadcast, multicast, documentation and benchmarking ranges, reserved
/// `240.0.0.0/4`, 6to4 and Teredo. Unless `private`, also loopback, RFC 1918,
/// link-local, CGNAT and IPv6 ULA / link-local. IPv4-mapped, -compatible or
/// NAT64 (`64:ff9b::/96`) IPv6 is classified as the embedded v4.
#[must_use]
pub fn is_blocked_ip(ip: IpAddr, private: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4, private),
        IpAddr::V6(v6) => is_blocked_ipv6(v6, private),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr, private: bool) -> bool {
    let o = ip.octets();
    let local = ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (o[0] == 100 && (64..128).contains(&o[1])); // CGNAT 100.64.0.0/10 (tailnets)
    matches!(o, [169, 254, 169, 254] | [169, 254, 170, 2] | [100, 100, 100, 200]) // metadata
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || o[0] == 0 // 0.0.0.0/8
        || matches!((o[0], o[1], o[2]), (192, 0, 2) | (198, 51, 100) | (203, 0, 113)) // TEST-NET
        || (o[0] == 198 && (18..20).contains(&o[1])) // benchmarking 198.18.0.0/15
        || o[0] >= 240 // reserved 240.0.0.0/4 (incl. broadcast)
        || (local && !private)
}

fn is_blocked_ipv6(ip: Ipv6Addr, private: bool) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4, private);
    }
    // Deprecated IPv4-compatible `::x.x.x.x` (but not `::` / `::1`).
    if let Some(v4) = ip.to_ipv4()
        && !ip.is_loopback()
        && !ip.is_unspecified()
    {
        return is_blocked_ipv4(v4, private);
    }
    let seg = ip.segments();
    // NAT64 well-known prefix 64:ff9b::/96 carries an IPv4 address in the low 32 bits.
    if seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_blocked_ipv4(
            Ipv4Addr::from((u32::from(seg[6]) << 16) | u32::from(seg[7])),
            private,
        );
    }
    let local = ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local();
    seg == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254] // AWS metadata fd00:ec2::254
        || ip.is_unspecified()
        || ip.is_multicast()
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation 2001:db8::/32
        // 183 Z3-R3: transition tunnels that reach an IPv4 address the checks above
        // never see: 6to4 2002::/16 and Teredo 2001::/32.
        || seg[0] == 0x2002
        || (seg[0] == 0x2001 && seg[1] == 0)
        || (local && !private)
}
