//! Per-cookie delta merge (209 §4.2): writers upload changes against the last state they saw, never
//! a site snapshot, so a stale browser cannot delete what another wrote. Changes are grouped by
//! registrable domain and applied to the current row: set replaces the key, remove deletes it, an
//! `expires` in the past deletes it. Pure functions; the store calls them under the actor.

use std::collections::{BTreeMap, HashMap};

use crate::{Cookie, CookieChange, CookieKey, OriginStorage, SiteJar, caps, site_of};

/// The key a cookie is matched by: `(name, domain without a leading '.', lower case, path)`.
fn norm(name: &str, domain: &str, path: &str) -> (String, String, String) {
    (
        name.to_owned(),
        domain.trim_start_matches('.').to_ascii_lowercase(),
        path.to_owned(),
    )
}

fn norm_cookie(c: &Cookie) -> (String, String, String) {
    norm(&c.name, &c.domain, &c.path)
}

/// `true` when `c` has expired at `now_s` (Unix seconds); session cookies never do.
pub fn expired(c: &Cookie, now_s: i64) -> bool {
    c.expires.is_some_and(|e| e <= now_s)
}

/// Group `changes` by the site of their domain (209 §4.2).
pub fn group(changes: Vec<CookieChange>) -> BTreeMap<String, Vec<CookieChange>> {
    let mut out: BTreeMap<String, Vec<CookieChange>> = BTreeMap::new();
    for c in changes {
        out.entry(site_of(c.domain())).or_default().push(c);
    }
    out
}

/// The host of `origin` (`scheme://host[:port]`), lower case.
pub fn origin_host(origin: &str) -> Option<String> {
    let rest = origin.split_once("://").map_or(origin, |(_, r)| r);
    let authority = rest.split('/').next()?;
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Group snapshots by the site of their origin's host; snapshots without a host are dropped.
pub fn group_storage(storage: Vec<OriginStorage>) -> BTreeMap<String, Vec<OriginStorage>> {
    let mut out: BTreeMap<String, Vec<OriginStorage>> = BTreeMap::new();
    for s in storage {
        if let Some(host) = origin_host(&s.origin) {
            out.entry(site_of(&host)).or_default().push(s);
        }
    }
    out
}

/// Apply cookie `changes` to `jar` at `now_s`; returns how many were applied.
pub fn apply(jar: &mut SiteJar, changes: &[CookieChange], now_s: i64) -> usize {
    for c in changes {
        match c {
            CookieChange::Set { cookie } => {
                let k = norm_cookie(cookie);
                jar.cookies.retain(|x| norm_cookie(x) != k);
                if !expired(cookie, now_s) {
                    jar.cookies.push(cookie.clone());
                }
            }
            CookieChange::Remove { name, domain, path } => {
                let k = norm(name, domain, path);
                jar.cookies.retain(|x| norm_cookie(x) != k);
            }
        }
    }
    changes.len()
}

/// Apply one localStorage snapshot: a newer capture replaces the origin's, an older one is ignored,
/// and one over [`caps::ORIGIN_STORAGE_BYTES`] is dropped whole together with the origin's older
/// one (never cut). Returns `true` when the jar changed.
pub fn apply_storage(jar: &mut SiteJar, snap: &OriginStorage) -> bool {
    let at = jar.origins.iter().position(|o| o.origin == snap.origin);
    if let Some(i) = at
        && jar.origins[i].captured_ms > snap.captured_ms
    {
        return false;
    }
    if let Some(i) = at {
        jar.origins.remove(i);
    }
    if snap.bytes() > caps::ORIGIN_STORAGE_BYTES {
        return at.is_some();
    }
    jar.origins.push(snap.clone());
    true
}

/// Drop expired cookies; with `idle` (the row went [`caps::SESSION_COOKIE_IDLE_MS`] without a
/// write, 209 §11 Q1) drop session cookies too.
pub fn prune(jar: &mut SiteJar, now_s: i64, idle: bool) {
    jar.cookies
        .retain(|c| !expired(c, now_s) && !(idle && c.expires.is_none()));
}

/// The delta that turns `before` into `after` (209 §4.2), keyed `(name, domain, path)`: a `set` for
/// every cookie new or changed in any attribute, a `remove` for every cookie gone.
pub fn diff(before: &[Cookie], after: &[Cookie]) -> Vec<CookieChange> {
    let old: HashMap<_, &Cookie> = before.iter().map(|c| (norm_cookie(c), c)).collect();
    let new: HashMap<_, &Cookie> = after.iter().map(|c| (norm_cookie(c), c)).collect();
    let mut out = Vec::new();
    for c in after {
        if old.get(&norm_cookie(c)).is_none_or(|o| *o != c) {
            out.push(CookieChange::Set { cookie: c.clone() });
        }
    }
    for c in before {
        if !new.contains_key(&norm_cookie(c)) {
            let CookieKey { name, domain, path } = c.key();
            out.push(CookieChange::Remove { name, domain, path });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str, value: &str, domain: &str) -> Cookie {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            host_only: false,
            path: "/".into(),
            expires: None,
            secure: true,
            http_only: true,
            same_site: "Lax".into(),
            partitioned: false,
        }
    }

    #[test]
    fn diff_sets_changed_and_removes_gone() {
        let a = cookie("a", "1", "example.com");
        let b = cookie("b", "1", "example.com");
        let b2 = cookie("b", "2", ".example.com");
        let c = cookie("c", "1", "example.com");
        let d = diff(&[a.clone(), b], &[b2.clone(), c.clone()]);
        assert_eq!(
            d,
            vec![
                CookieChange::Set { cookie: b2 },
                CookieChange::Set { cookie: c },
                CookieChange::Remove {
                    name: "a".into(),
                    domain: "example.com".into(),
                    path: "/".into()
                },
            ]
        );
        assert!(diff(&[a.clone()], &[a]).is_empty());
    }

    #[test]
    fn past_expiry_removes_and_groups_by_site() {
        let mut jar = SiteJar::new("example.com");
        apply(
            &mut jar,
            &[CookieChange::Set {
                cookie: cookie("a", "1", "app.example.com"),
            }],
            100,
        );
        let mut gone = cookie("a", "x", "app.example.com");
        gone.expires = Some(50);
        apply(&mut jar, &[CookieChange::Set { cookie: gone }], 100);
        assert!(jar.cookies.is_empty());
        let g = group(vec![
            CookieChange::Set {
                cookie: cookie("a", "1", ".app.example.co.uk"),
            },
            CookieChange::Set {
                cookie: cookie("b", "1", "www.example.com"),
            },
        ]);
        assert_eq!(
            g.keys().collect::<Vec<_>>(),
            ["example.co.uk", "example.com"]
        );
        assert_eq!(origin_host("https://[::1]:8443").as_deref(), Some("::1"));
        assert_eq!(
            origin_host("https://App.Example.com:443").as_deref(),
            Some("app.example.com")
        );
    }
}
