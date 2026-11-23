//! 210 §7.2 (F-m7): the origins of theuth's own web client, which neither `browser_*` nor
//! `computer_*` may drive while a human-only approval is pending.

use std::net::{IpAddr, SocketAddr};

use runtime::Runtime;

/// The engine's own web-client origins: the RPC listener (`rpc.listen`, which serves the web
/// client) and the link relay (`link.relay`, whose host serves the paired web app).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnOrigins {
    listen: Option<SocketAddr>,
    /// Host (lowercase) and port of the relay's web app.
    relay: Option<(String, u16)>,
}

impl OwnOrigins {
    /// From `rpc.listen` (default `127.0.0.1:7777`, like `rpc::RpcConfig`) and `link.relay`
    /// (default `wss://relay.theuth.io`, like `link::LinkConfig`).
    #[must_use]
    pub fn from_config(rt: &Runtime) -> Self {
        let get = |key: &str| {
            rt.config
                .find_value(key)
                .ok()
                .and_then(|v| v.into_string())
        };
        let listen = get("rpc.listen").unwrap_or_else(|| "127.0.0.1:7777".to_owned());
        let relay = get("link.relay").unwrap_or_else(|| "wss://relay.theuth.io".to_owned());
        Self::new(&listen, Some(&relay))
    }

    /// `listen` is `host:port`; `relay` a `ws(s)://` or `http(s)://` origin.
    #[must_use]
    pub fn new(listen: &str, relay: Option<&str>) -> Self {
        Self {
            listen: listen.parse().ok(),
            relay: relay.and_then(host_port),
        }
    }

    /// Whether `url` (http, https, ws or wss) is served by theuth's own web client. On the
    /// listener's port: an unspecified listen address (`0.0.0.0`, `::`) matches every host (a
    /// LAN address, an mDNS name), a loopback one every loopback name, any other its own IP.
    /// The relay matches on host and port.
    #[must_use]
    pub fn contains(&self, url: &str) -> bool {
        let Ok(u) = url::Url::parse(url) else {
            return false;
        };
        if !matches!(u.scheme(), "http" | "https" | "ws" | "wss") {
            return false;
        }
        let port = u.port_or_known_default();
        if let Some(addr) = self.listen
            && port == Some(addr.port())
            && listener_host(addr.ip(), u.host())
        {
            return true;
        }
        self.relay.as_ref().is_some_and(|(host, p)| {
            Some(*p) == port && u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(host))
        })
    }
}

/// Whether `host` reaches a listener bound to `ip`.
fn listener_host(ip: IpAddr, host: Option<url::Host<&str>>) -> bool {
    if ip.is_unspecified() {
        return host.is_some();
    }
    let loopback = ip.is_loopback();
    let ip_ok = |h: IpAddr| h == ip || (loopback && (h.is_loopback() || h.is_unspecified()));
    match host {
        Some(url::Host::Ipv4(h)) => ip_ok(h.into()),
        Some(url::Host::Ipv6(h)) => ip_ok(h.into()),
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            loopback && (d == "localhost" || d.ends_with(".localhost"))
        }
        None => false,
    }
}

fn host_port(origin: &str) -> Option<(String, u16)> {
    let u = url::Url::parse(origin).ok()?;
    Some((u.host_str()?.to_ascii_lowercase(), u.port_or_known_default()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY: Option<&str> = Some("wss://relay.theuth.io");

    /// The listener under every loopback name, the relay's web app; nothing else.
    #[test]
    fn loopback_listener_and_relay() {
        let own = OwnOrigins::new("127.0.0.1:7777", RELAY);
        for url in [
            "http://127.0.0.1:7777/",
            "http://localhost:7777/#/settings/sharing",
            "http://LOCALHOST.:7777/",
            "ws://[::1]:7777/rpc",
            "https://relay.theuth.io/link#1.abc",
            "https://Relay.Theuth.IO/d/x",
        ] {
            assert!(own.contains(url), "{url}");
        }
        for url in [
            "http://127.0.0.1:8080/",
            "http://192.168.1.5:7777/",
            "https://example.com:7777/",
            "https://relay.theuth.io:8443/",
            "https://evil.relay.theuth.io.example/",
            "about:blank",
            "file:///tmp/x",
        ] {
            assert!(!own.contains(url), "{url}");
        }
    }

    /// `rpc.listen: 0.0.0.0:<port>` serves every interface: a LAN IP or an mDNS name on that
    /// port is theuth's own web client too.
    #[test]
    fn unspecified_listener_matches_every_host_on_its_port() {
        for listen in ["0.0.0.0:7777", "[::]:7777"] {
            let own = OwnOrigins::new(listen, None);
            for url in [
                "http://192.168.1.5:7777/",
                "http://my-mac.local:7777/",
                "http://localhost:7777/",
                "ws://[fe80::1]:7777/rpc",
            ] {
                assert!(own.contains(url), "{listen} {url}");
            }
            assert!(!own.contains("http://192.168.1.5:7778/"), "{listen}");
        }
    }

    /// A listener on one LAN address matches that address only.
    #[test]
    fn specific_listener_matches_its_own_ip() {
        let own = OwnOrigins::new("192.168.1.5:7777", None);
        assert!(own.contains("http://192.168.1.5:7777/"));
        assert!(!own.contains("http://192.168.1.6:7777/"));
        assert!(!own.contains("http://localhost:7777/"));
    }
}
