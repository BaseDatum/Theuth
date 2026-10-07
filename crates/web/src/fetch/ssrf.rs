//! SSRF guard for `read_url` (design doc 16 §5): scheme / credential checks,
//! a hostname denylist, IP-range classification, and DNS resolve-and-check.
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
/// any non-global address.
pub async fn check_url(raw: &str) -> Result<Url, FetchError> {
    let url = parse_absolute(raw)?;
    check_static(&url, &[])?;
    if let Some(Host::Domain(domain)) = url.host() {
        resolve_checked(domain).await?;
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
pub(super) fn check_static(url: &Url, allow: &[SocketAddr]) -> Result<(), FetchError> {
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
        Host::Domain(domain) if is_blocked_hostname(domain) => Err(FetchError::Rejected(format!(
            "hostname `{domain}` is not allowed"
        ))),
        Host::Domain(_) => Ok(()),
        Host::Ipv4(ip) => assert_ip_allowed(IpAddr::V4(ip), port, allow),
        Host::Ipv6(ip) => assert_ip_allowed(IpAddr::V6(ip), port, allow),
    }
}

fn assert_ip_allowed(ip: IpAddr, port: u16, allow: &[SocketAddr]) -> Result<(), FetchError> {
    if !is_blocked_ip(ip) || allow.contains(&SocketAddr::new(ip, port)) {
        return Ok(());
    }
    Err(FetchError::Rejected(format!(
        "IP address `{ip}` is not allowed"
    )))
}

/// Resolve `host` and require **every** returned address to be global.
async fn resolve_checked(host: &str) -> Result<Vec<SocketAddr>, FetchError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|e| FetchError::Rejected(format!("DNS resolve failed for `{host}`: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(FetchError::Rejected(format!(
            "DNS returned no addresses for `{host}`"
        )));
    }
    if let Some(bad) = addrs.iter().find(|a| is_blocked_ip(a.ip())) {
        return Err(FetchError::Rejected(format!(
            "hostname `{host}` resolves to blocked address `{}`",
            bad.ip()
        )));
    }
    Ok(addrs)
}

/// `reqwest` DNS resolver that refuses non-global addresses, so the client can
/// only ever connect to what [`is_blocked_ip`] cleared. IP literals never reach
/// a resolver; [`check_static`] covers those.
pub(super) struct GuardedResolver;

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs = resolve_checked(name.as_str()).await?;
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

/// Whether a hostname is clearly non-global without DNS: `localhost`,
/// `*.localhost`, `*.local` (mDNS), `*.internal`, `*.lan`, `*.home.arpa`,
/// `*.intranet`, `*.corp` (and each as a bare label), and common cloud
/// metadata aliases.
#[must_use]
pub fn is_blocked_hostname(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h.is_empty()
        || h == "localhost"
        || INTERNAL_SUFFIXES
            .iter()
            .any(|s| h.ends_with(s) || h == &s[1..])
        || matches!(
            h.as_str(),
            "metadata.google.internal"
                | "metadata"
                | "kubernetes.default"
                | "kubernetes.default.svc"
                | "kubernetes.default.svc.cluster.local"
        )
}

/// Whether an IP must not be contacted.
///
/// Loopback, RFC 1918, link-local (incl. `169.254.169.254`), CGNAT, multicast,
/// unspecified, documentation and benchmarking ranges, reserved `240.0.0.0/4`,
/// IPv6 ULA / link-local, and IPv4-mapped, -compatible or NAT64
/// (`64:ff9b::/96`) IPv6 (classified as the embedded v4).
#[must_use]
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || o[0] == 0 // 0.0.0.0/8
        || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT 100.64.0.0/10
        || matches!((o[0], o[1], o[2]), (192, 0, 2) | (198, 51, 100) | (203, 0, 113)) // TEST-NET
        || (o[0] == 198 && (18..20).contains(&o[1])) // benchmarking 198.18.0.0/15
        || o[0] >= 240 // reserved 240.0.0.0/4 (incl. broadcast)
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4);
    }
    // Deprecated IPv4-compatible `::x.x.x.x` (but not `::` / `::1`).
    if let Some(v4) = ip.to_ipv4()
        && !ip.is_loopback()
        && !ip.is_unspecified()
    {
        return is_blocked_ipv4(v4);
    }
    let seg = ip.segments();
    // NAT64 well-known prefix 64:ff9b::/96 carries an IPv4 address in the low 32 bits.
    if seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_blocked_ipv4(Ipv4Addr::from(
            (u32::from(seg[6]) << 16) | u32::from(seg[7]),
        ));
    }
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation 2001:db8::/32
}
