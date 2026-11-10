//! 183 §7.1 (W-20): LAN discovery for contacts. While an Add contact window is open this
//! engine advertises the first of `a2a.mdns_types` (draft-zhao `_a2a._tcp`) with TXT `card`,
//! `proto`, `v`, the theuth hints `eid`, `pair=1` and `port` (the iroh UDP port; addresses
//! come from the sighting's own A/AAAA records, and no login name is published), and
//! browses every type in the list so other vendors' agents show too. Nothing is advertised
//! or browsed outside a window, and a sighting is a hint: nothing is stored from it.
//!
//! mDNS sits behind [`Mdns`]: [`Swarm`] on a real network (it also serves the signed card on
//! a LAN port for the advert's `card` path), an in-process [`fake::Lan`] in tests.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

/// One advert this engine publishes while a window is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advert {
    /// The LAN addresses the card is served on (not published as TXT).
    pub ips: Vec<std::net::IpAddr>,
    /// Service type (`_a2a._tcp`).
    pub service: String,
    /// Instance label (the card name, made a DNS label, plus the id hint).
    pub instance: String,
    /// TXT pairs.
    pub txt: Vec<(String, String)>,
    /// The signed public card (JSON), served at TXT `card` by a real advert.
    pub card: String,
}

/// One browse result: an agent seen (`gone: false`) or expired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sighting {
    /// The service type it was seen under.
    pub service: String,
    /// Its instance label.
    pub instance: String,
    /// Where it answered.
    pub addrs: Vec<SocketAddr>,
    /// Its TXT pairs.
    pub txt: BTreeMap<String, String>,
    /// It went away.
    pub gone: bool,
}

/// A browse callback.
pub type Seen = Box<dyn Fn(Sighting) + Send + Sync>;

/// mDNS advertise + browse: `start` advertises `advert` and reports every sighting of
/// `browse` (its own advert excluded) until the returned guard is dropped.
pub trait Mdns: Send + Sync {
    /// Start both; drop the guard to stop them.
    fn start(
        &self,
        advert: Advert,
        browse: Vec<String>,
        seen: Seen,
    ) -> Result<Box<dyn Send>, String>;
}

/// `_a2a._tcp` → (`a2a`, TCP).
fn split(service: &str) -> Option<(String, swarm_discovery::Protocol)> {
    let (name, proto) = service.strip_prefix('_')?.split_once("._")?;
    let proto = match proto.trim_end_matches('.') {
        "tcp" => swarm_discovery::Protocol::Tcp,
        "udp" => swarm_discovery::Protocol::Udp,
        _ => return None,
    };
    Some((name.to_owned(), proto))
}

/// Concurrent connections the card listener serves.
const CARD_CONNS_MAX: usize = 8;

/// Serve the card on `std_listener`, one HTTP/1.1 request per connection, within
/// [`CARD_CONNS_MAX`] live connections of `pool` (more are dropped at accept) and with
/// header and whole-connection deadlines.
async fn serve_card(
    std_listener: std::net::TcpListener,
    app: axum::Router,
    pool: Arc<tokio::sync::Semaphore>,
) {
    use tower::ServiceExt as _;
    let Ok(listener) = tokio::net::TcpListener::from_std(std_listener) else {
        return;
    };
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        };
        let Ok(permit) = pool.clone().try_acquire_owned() else {
            continue;
        };
        let app = app.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let svc =
                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    app.clone().oneshot(req.map(axum::body::Body::new))
                });
            let conn = hyper::server::conn::http1::Builder::new()
                .timer(hyper_util::rt::TokioTimer::new())
                .header_read_timeout(std::time::Duration::from_secs(10))
                .keep_alive(false)
                .serve_connection(hyper_util::rt::TokioIo::new(sock), svc);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(30), conn).await;
        });
    }
}

/// Real mDNS (`swarm-discovery`, as the mesh's Nearby uses), one discoverer per service
/// type; the advertised one points at a LAN listener that serves only the signed card.
pub struct Swarm;

impl Mdns for Swarm {
    fn start(
        &self,
        advert: Advert,
        browse: Vec<String>,
        seen: Seen,
    ) -> Result<Box<dyn Send>, String> {
        let handle = tokio::runtime::Handle::current();
        let seen = Arc::new(seen);
        // The card listener: GET /.well-known/agent-card.json, nothing else. It binds only
        // the LAN addresses the advert carries (never 0.0.0.0), at most
        // [`CARD_CONNS_MAX`] connections at once; without a LAN address nothing is
        // advertised.
        let mut listeners = Vec::new();
        let mut port = 0u16;
        for ip in advert
            .ips
            .iter()
            .filter(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
        {
            match std::net::TcpListener::bind(SocketAddr::new(*ip, port)) {
                Ok(l) => {
                    l.set_nonblocking(true).map_err(|e| e.to_string())?;
                    port = l.local_addr().map_err(|e| e.to_string())?.port();
                    listeners.push((*ip, l));
                }
                Err(e) => tracing::debug!(%ip, "a2a: card listener: {e}"),
            }
        }
        if listeners.is_empty() {
            return Err("no LAN address to serve the card on".into());
        }
        let ips: Vec<std::net::IpAddr> = listeners.iter().map(|(ip, _)| *ip).collect();
        let card = advert.card.clone();
        let app = axum::Router::new().route(
            crate::config::CARD_SUFFIX,
            axum::routing::get(move || {
                let card = card.clone();
                async move {
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        card,
                    )
                }
            }),
        );
        let pool = Arc::new(tokio::sync::Semaphore::new(CARD_CONNS_MAX));
        let serve: Vec<_> = listeners
            .into_iter()
            .map(|(_, l)| handle.spawn(serve_card(l, app.clone(), pool.clone())))
            .collect();
        let mut guards = Vec::new();
        for service in &browse {
            let Some((name, proto)) = split(service) else {
                continue;
            };
            let mine = *service == advert.service;
            let label = if mine {
                advert.instance.clone()
            } else {
                format!("{}-browse", advert.instance)
            };
            let svc = service.clone();
            let seen = seen.clone();
            let own = advert.instance.clone();
            let mut d = swarm_discovery::Discoverer::new_interactive(name, label)
                .with_protocol(proto)
                .with_callback(move |from: &str, peer: &swarm_discovery::Peer| {
                    if from == own {
                        return;
                    }
                    (*seen)(Sighting {
                        service: svc.clone(),
                        instance: from.to_owned(),
                        addrs: peer
                            .addrs()
                            .iter()
                            .map(|&(ip, port)| SocketAddr::new(ip, port))
                            .collect(),
                        txt: peer
                            .txt_attributes()
                            .map(|(k, v)| (k.to_owned(), v.unwrap_or_default().to_owned()))
                            .collect(),
                        gone: peer.is_expiry(),
                    });
                });
            if mine {
                d = d
                    .with_txt_attributes(
                        advert.txt.iter().map(|(k, v)| (k.clone(), Some(v.clone()))),
                    )
                    .map_err(|e| e.to_string())?
                    .with_addrs(port, ips.clone());
            }
            guards.push(
                d.spawn(&handle)
                    .map_err(|e| format!("mDNS {service}: {e}"))?,
            );
        }
        /// Dropping it stops the discoverers and the card listeners.
        struct Guard {
            _mdns: Vec<swarm_discovery::DropGuard>,
            serve: Vec<tokio::task::JoinHandle<()>>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                for s in &self.serve {
                    s.abort();
                }
            }
        }
        Ok(Box::new(Guard {
            _mdns: guards,
            serve,
        }))
    }
}

/// Real mDNS, or in a debug build with `THEUTH_DEV_CONTACTS_DIR` set, [`dev::DirLan`]: the
/// scratch-engine rig's adverts as files and the card on loopback, so the Nearby agents
/// list is proven without any network interface (swarm-discovery always multicasts on the
/// default interface, so it cannot be held to `lo0`).
pub fn default_mdns() -> Arc<dyn Mdns> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("THEUTH_DEV_CONTACTS_DIR") {
        tracing::warn!("a2a: Nearby agents use the dev directory {dir:?}, not mDNS");
        return Arc::new(dev::DirLan(std::path::PathBuf::from(dir)));
    }
    Arc::new(Swarm)
}

#[cfg(debug_assertions)]
mod dev {
    use super::*;

    /// One advert per file (JSON); the card is served on `127.0.0.1` only.
    pub(super) struct DirLan(pub(super) std::path::PathBuf);

    struct Member {
        file: std::path::PathBuf,
        tasks: [tokio::task::JoinHandle<()>; 2],
    }

    impl Drop for Member {
        fn drop(&mut self) {
            for t in &self.tasks {
                t.abort();
            }
            let _ = std::fs::remove_file(&self.file);
        }
    }

    fn read(text: &str) -> Option<Sighting> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        Some(Sighting {
            service: v["service"].as_str()?.to_owned(),
            instance: v["instance"].as_str()?.to_owned(),
            addrs: v["addrs"]
                .as_array()?
                .iter()
                .filter_map(|a| a.as_str()?.parse().ok())
                .collect(),
            txt: serde_json::from_value(v["txt"].clone()).ok()?,
            gone: false,
        })
    }

    impl Mdns for DirLan {
        fn start(
            &self,
            advert: Advert,
            browse: Vec<String>,
            seen: Seen,
        ) -> Result<Box<dyn Send>, String> {
            std::fs::create_dir_all(&self.0).map_err(|e| format!("{:?}: {e}", self.0))?;
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .map_err(|e| format!("card listener: {e}"))?;
            listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            let addr = listener.local_addr().map_err(|e| e.to_string())?;
            let card = advert.card.clone();
            let serve = tokio::spawn(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                    return;
                };
                let app = axum::Router::new().route(
                    crate::config::CARD_SUFFIX,
                    axum::routing::get(move || {
                        let card = card.clone();
                        async move {
                            (
                                [(axum::http::header::CONTENT_TYPE, "application/json")],
                                card,
                            )
                        }
                    }),
                );
                let _ = axum::serve(listener, app).await;
            });
            // One file per advert (the card port makes it unique), so a stale guard's drop
            // never removes a newer window's advert.
            let file = self.0.join(format!("{}.{}", advert.instance, addr.port()));
            let txt: BTreeMap<String, String> = advert.txt.iter().cloned().collect();
            let body = serde_json::json!({
                "service": advert.service,
                "instance": advert.instance,
                "addrs": [addr.to_string()],
                "txt": txt,
            });
            std::fs::write(&file, body.to_string()).map_err(|e| format!("{file:?}: {e}"))?;
            let (dir, own) = (self.0.clone(), advert.instance.clone());
            let watch = tokio::spawn(async move {
                let mut known: BTreeMap<String, Sighting> = BTreeMap::new();
                loop {
                    let now: BTreeMap<String, Sighting> = std::fs::read_dir(&dir)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .filter_map(|e| read(&std::fs::read_to_string(e.path()).ok()?))
                        .filter(|s| s.instance != own && browse.contains(&s.service))
                        .map(|s| (s.instance.clone(), s))
                        .collect();
                    for (k, s) in &now {
                        if known.get(k) != Some(s) {
                            seen(s.clone());
                        }
                    }
                    for (k, s) in &known {
                        if !now.contains_key(k) {
                            seen(Sighting {
                                gone: true,
                                ..s.clone()
                            });
                        }
                    }
                    known = now;
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            });
            Ok(Box::new(Member {
                file,
                tasks: [serve, watch],
            }))
        }
    }
}

/// The adverts this engine publishes for `card` (JSON) while a window is open: §7.1's
/// fields only (`card`, `proto`, `v`, the `eid` hint, `pair`) plus `port`, the iroh UDP port
/// (the addresses come from the sighting's own A/AAAA records). `machine` (the mesh name,
/// never the login) labels the instance; `direct` are this engine's iroh addresses, whose
/// IPs the card is served on.
pub fn advert(service: &str, machine: &str, eid: &str, direct: &[String], card: String) -> Advert {
    let label: String = crate::config::alias_of(machine);
    let hint: String = eid.chars().take(16).collect();
    let direct: Vec<SocketAddr> = direct.iter().filter_map(|a| a.parse().ok()).collect();
    let mut ips: Vec<std::net::IpAddr> = Vec::new();
    for a in &direct {
        if !ips.contains(&a.ip()) {
            ips.push(a.ip());
        }
    }
    let mut txt = vec![
        ("card".into(), crate::config::CARD_SUFFIX.into()),
        ("proto".into(), super::BINDING_IROH.into()),
        ("v".into(), "1.0".into()),
        ("eid".into(), hint.clone()),
        ("pair".into(), "1".into()),
    ];
    if let Some(a) = direct.first() {
        txt.push(("port".into(), a.port().to_string()));
    }
    Advert {
        service: service.to_owned(),
        instance: format!("{label}-{}", &hint[..hint.len().min(8)]),
        txt,
        ips,
        card,
    }
}

/// A sighting as `contact_nearby` lists it. The card URL is always the sighting's own
/// address plus the TXT path: an absolute URL in the TXT keeps its path only, never its
/// host (a LAN neighbour cannot point the fetch elsewhere).
pub fn nearby(s: &Sighting) -> super::NearbyAgent {
    let addr = s.addrs.first().map(ToString::to_string).unwrap_or_default();
    let path = s
        .txt
        .get("card")
        .or_else(|| s.txt.get("cardUrl"))
        .or_else(|| s.txt.get("path"))
        .and_then(|p| {
            if p.starts_with("http://") || p.starts_with("https://") {
                let u = url::Url::parse(p).ok()?;
                Some(match u.query() {
                    Some(q) => format!("{}?{q}", u.path()),
                    None => u.path().to_owned(),
                })
            } else {
                Some(p.clone())
            }
        })
        .filter(|p| p.starts_with('/'));
    let card = path.filter(|_| !addr.is_empty()).map(|p| {
        let scheme = if s.txt.get("tls").map(String::as_str) == Some("1") {
            "https"
        } else {
            "http"
        };
        format!("{scheme}://{addr}{p}")
    });
    super::NearbyAgent {
        name: s.instance.clone(),
        service: s.service.clone(),
        addr,
        eid: s.txt.get("eid").cloned().filter(|e| !e.is_empty()),
        card,
        verified: false,
    }
}

/// Where to dial a theuth advert's iroh endpoint: the sighting's own addresses with the
/// iroh UDP port of its `port` hint (none without one).
pub fn iroh_addrs(s: &Sighting) -> Vec<SocketAddr> {
    let Some(port) = s.txt.get("port").and_then(|p| p.parse::<u16>().ok()) else {
        return Vec::new();
    };
    s.addrs
        .iter()
        .map(|a| SocketAddr::new(a.ip(), port))
        .collect()
}

/// An in-process LAN for tests: every member sees every other member's advert under the
/// types it browses, and a dropped guard is announced gone. No socket is opened.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use parking_lot::Mutex;

    type Member = (u64, Advert, Vec<String>, Arc<Seen>);

    #[derive(Clone, Default)]
    pub(crate) struct Lan(Arc<Mutex<(u64, Vec<Member>)>>);

    impl Lan {
        /// Adverts live right now.
        pub(crate) fn adverts(&self) -> Vec<Advert> {
            self.0.lock().1.iter().map(|m| m.1.clone()).collect()
        }
    }

    fn sighting(a: &Advert, gone: bool) -> Sighting {
        Sighting {
            service: a.service.clone(),
            instance: a.instance.clone(),
            // The advert's own addresses stand in for the A/AAAA records.
            addrs: a
                .ips
                .iter()
                .map(|ip| SocketAddr::new(*ip, 5353))
                .chain(
                    a.ips
                        .is_empty()
                        .then(|| "192.168.1.9:5353".parse().unwrap()),
                )
                .collect(),
            txt: a.txt.iter().cloned().collect(),
            gone,
        }
    }

    struct Guard(Lan, u64);

    impl Drop for Guard {
        fn drop(&mut self) {
            let (gone, others) = {
                let mut m = self.0.0.lock();
                let gone = m.1.iter().find(|x| x.0 == self.1).map(|x| x.1.clone());
                m.1.retain(|x| x.0 != self.1);
                (gone, m.1.clone())
            };
            if let Some(a) = gone {
                for (_, _, browse, seen) in others {
                    if browse.contains(&a.service) {
                        (*seen)(sighting(&a, true));
                    }
                }
            }
        }
    }

    impl Mdns for Lan {
        fn start(
            &self,
            advert: Advert,
            browse: Vec<String>,
            seen: Seen,
        ) -> Result<Box<dyn Send>, String> {
            let seen: Arc<Seen> = Arc::new(seen);
            let (key, others) = {
                let mut m = self.0.lock();
                m.0 += 1;
                let key = m.0;
                let others = m.1.clone();
                m.1.push((key, advert.clone(), browse.clone(), seen.clone()));
                (key, others)
            };
            for (_, a, their_browse, their_seen) in others {
                if browse.contains(&a.service) {
                    (*seen)(sighting(&a, false));
                }
                if their_browse.contains(&advert.service) {
                    (*their_seen)(sighting(&advert, false));
                }
            }
            Ok(Box::new(Guard(self.clone(), key)))
        }
    }
}
