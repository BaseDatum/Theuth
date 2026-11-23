//! The hosting engine's routing table (210 §5.4): `host → [(path prefix, mount)]`, longest prefix
//! wins; a host without an entry is `421`, a host without a `/` share a plain 404.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::model::{Access, Share, Source};
use crate::sources::DirPin;

/// One share as the proxy serves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// The share id.
    pub share: String,
    /// Public host.
    pub host: String,
    /// `""` for `/`, else `/app1` (no trailing slash).
    pub prefix: String,
    /// What it serves.
    pub source: Source,
    /// Who may open it.
    pub access: Access,
    /// Interstitial on.
    pub interstitial: bool,
    /// The edge's domain `D` (names are `<name>.D`).
    pub domain: String,
    /// A `dir` source's root as approved at publish (canonical path, device, inode); `None` for
    /// a `port` source (a `dir` mount without a pin answers 404).
    pub dir: Option<DirPin>,
}

impl Mount {
    /// From a share row and, for a `dir` source, the pin stored at publish. Pinning and the
    /// protected-path checks happen in the engine against its own data dir, never here.
    pub fn of(s: &Share, dir: Option<DirPin>) -> Self {
        let dir = match &s.source {
            Source::Dir(_) => dir,
            Source::Port(_) => None,
        };
        Self {
            share: s.id.clone(),
            host: s.host.clone(),
            prefix: if s.path == "/" {
                String::new()
            } else {
                s.path.clone()
            },
            source: s.source.clone(),
            access: s.access,
            interstitial: s.interstitial,
            domain: s.domain.clone(),
            dir,
        }
    }

    /// The name host (`<name>.D`) `host` belongs to under this mount's domain, lowercased;
    /// `None` when `host` is not below the domain.
    pub fn name_of(&self, host: &str) -> Option<String> {
        let host = normalize_host(host);
        let domain = self.domain.trim_end_matches('.').to_ascii_lowercase();
        let below = host.strip_suffix(&domain)?.strip_suffix('.')?;
        let name = below.rsplit('.').next().filter(|n| !n.is_empty())?;
        Some(format!("{name}.{domain}"))
    }

    /// `https://{host}{prefix}`, no trailing slash.
    pub fn origin_with_prefix(&self) -> String {
        format!("https://{}{}", self.host, self.prefix)
    }
}

/// Result of a lookup.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// This engine does not route the host: `421 Misdirected Request`.
    Misdirected,
    /// Host known, no mount covers the path: 404.
    NotFound,
    /// The mount and the path below its prefix (starts with `/`).
    Found(Arc<Mount>, String),
}

/// Lock-free table read per request, swapped on publish and stop.
#[derive(Default)]
pub struct Table {
    map: ArcSwap<HashMap<String, Vec<Arc<Mount>>>>,
}

impl Table {
    /// Add or replace a mount.
    pub fn insert(&self, m: Mount) {
        self.map.rcu(|cur| {
            let mut next = (**cur).clone();
            for v in next.values_mut() {
                v.retain(|x| x.share != m.share);
            }
            let list = next.entry(m.host.to_ascii_lowercase()).or_default();
            list.push(Arc::new(m.clone()));
            list.sort_by_key(|x| std::cmp::Reverse(x.prefix.len()));
            next.retain(|_, v| !v.is_empty());
            next
        });
    }

    /// Remove a share; returns whether it was routed.
    pub fn remove(&self, share: &str) -> bool {
        let mut hit = false;
        self.map.rcu(|cur| {
            let mut next = (**cur).clone();
            hit = false;
            for v in next.values_mut() {
                let n = v.len();
                v.retain(|x| x.share != share);
                hit |= v.len() != n;
            }
            next.retain(|_, v| !v.is_empty());
            next
        });
        hit
    }

    /// Hosts this engine routes: the full set for `PUT /v1/routes`.
    pub fn hosts(&self) -> Vec<String> {
        let mut h: Vec<String> = self.map.load().keys().cloned().collect();
        h.sort();
        h
    }

    /// Route `host` (`Host`/`:authority`, port and trailing dot ignored) and `path`.
    pub fn lookup(&self, host: &str, path: &str) -> Lookup {
        let host = normalize_host(host);
        let map = self.map.load();
        let Some(list) = map.get(&host) else {
            return Lookup::Misdirected;
        };
        for m in list {
            if m.prefix.is_empty() {
                return Lookup::Found(m.clone(), path.to_owned());
            }
            if let Some(rest) = path.strip_prefix(&m.prefix)
                && (rest.is_empty() || rest.starts_with('/'))
            {
                let rest = if rest.is_empty() { "/" } else { rest };
                return Lookup::Found(m.clone(), rest.to_owned());
            }
        }
        Lookup::NotFound
    }
}

/// Lowercase, without port and trailing dot.
pub fn normalize_host(host: &str) -> String {
    let h = match host.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) && !h.contains(':') => h,
        _ => host,
    };
    h.trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(share: &str, host: &str, prefix: &str) -> Mount {
        Mount {
            share: share.into(),
            host: host.into(),
            prefix: prefix.into(),
            source: Source::Dir("/srv".into()),
            access: Access::Public,
            interstitial: false,
            domain: "tun.zip".into(),
            dir: None,
        }
    }

    #[test]
    fn name_of_host() {
        let mt = m("a", "app.sky.tun.zip", "");
        assert_eq!(mt.name_of("sky.tun.zip").as_deref(), Some("sky.tun.zip"));
        assert_eq!(
            mt.name_of("APP.sky.tun.zip.:443").as_deref(),
            Some("sky.tun.zip")
        );
        assert_eq!(mt.name_of("other.tun.zip").as_deref(), Some("other.tun.zip"));
        assert_eq!(mt.name_of("tun.zip"), None);
        assert_eq!(mt.name_of("sky.example.org"), None);
        assert_eq!(mt.name_of("xtun.zip"), None);
    }

    #[test]
    fn longest_prefix_421_and_404() {
        let t = Table::default();
        t.insert(m("a", "sky.tun.zip", "/app1"));
        t.insert(m("b", "sky.tun.zip", "/app10"));
        t.insert(m("c", "app.sky.tun.zip", ""));
        match t.lookup("SKY.tun.zip:443", "/app1/x") {
            Lookup::Found(mt, rest) => assert_eq!((mt.share.as_str(), rest.as_str()), ("a", "/x")),
            o => panic!("{o:?}"),
        }
        match t.lookup("sky.tun.zip", "/app10") {
            Lookup::Found(mt, rest) => assert_eq!((mt.share.as_str(), rest.as_str()), ("b", "/")),
            o => panic!("{o:?}"),
        }
        assert_eq!(t.lookup("sky.tun.zip", "/app2"), Lookup::NotFound);
        assert_eq!(t.lookup("docs.sky.tun.zip", "/"), Lookup::Misdirected);
        assert!(t.remove("c"));
        assert_eq!(t.lookup("app.sky.tun.zip", "/"), Lookup::Misdirected);
        assert_eq!(t.hosts(), vec!["sky.tun.zip".to_owned()]);
    }
}
