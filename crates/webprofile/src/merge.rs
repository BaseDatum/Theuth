//! Per-cookie deltas (209 §4.2): writers upload changes against the last state they saw, never a
//! site snapshot, so a stale browser cannot delete what another wrote. Changes are checked
//! ([`validate`]), grouped by registrable domain ([`group`]) and each becomes a write of its own
//! row (one row per cookie, `crate::store`): set replaces the key, remove writes a tombstone, an
//! `expires` in the past removes it. Pure functions; the store calls them under the actor.

use std::collections::{BTreeMap, HashMap};

use crate::{
    Cookie, CookieChange, CookieKey, OriginStorage, ProfileError, Push, caps, cookie_host, site_of,
};

/// Characters of a push's writer label kept: the longest prefix (`obscura:`) and
/// [`caps::DEVICE_CHARS`].
const WRITER_CHARS: usize = 8 + caps::DEVICE_CHARS;

/// The key a cookie is matched by: `(name, domain without a leading '.', lower case, path)`.
pub fn norm(name: &str, domain: &str, path: &str) -> (String, String, String) {
    (
        name.to_owned(),
        domain.trim_start_matches('.').to_ascii_lowercase(),
        path.to_owned(),
    )
}

/// [`norm`] of a cookie.
pub fn norm_cookie(c: &Cookie) -> (String, String, String) {
    norm(&c.name, &c.domain, &c.path)
}

/// `true` when `c` has expired at `now_s` (Unix seconds); session cookies never do.
pub fn expired(c: &Cookie, now_s: i64) -> bool {
    c.expires.is_some_and(|e| e <= now_s)
}

/// Check and normalise `msg` before any work (209 §4.4): at most [`caps::PUSH_CHANGES`] changes
/// and [`caps::PUSH_STORAGE`] snapshots; the writer cut to its prefix and
/// [`caps::DEVICE_CHARS`] characters; every cookie with a name, a path from `/`, a value of at
/// most [`caps::COOKIE_VALUE`] bytes, and a domain cookies may be scoped to ([`cookie_host`]),
/// written lower case without a leading `.` (a leading `.` makes the cookie `host_only: false`).
/// Errors name the change by index, never a value.
pub fn validate(msg: &mut Push) -> Result<(), ProfileError> {
    if msg.changes.len() > caps::PUSH_CHANGES {
        return Err(ProfileError::Invalid(format!(
            "{} changes > {}",
            msg.changes.len(),
            caps::PUSH_CHANGES
        )));
    }
    if msg.storage.len() > caps::PUSH_STORAGE {
        return Err(ProfileError::Invalid(format!(
            "{} storage snapshots > {}",
            msg.storage.len(),
            caps::PUSH_STORAGE
        )));
    }
    if msg.writer.chars().count() > WRITER_CHARS {
        msg.writer = msg.writer.chars().take(WRITER_CHARS).collect();
    }
    for (i, change) in msg.changes.iter_mut().enumerate() {
        let bad = |why: &str| ProfileError::Invalid(format!("change {i}: {why}"));
        let (name, domain, path) = match change {
            CookieChange::Set { cookie } => {
                if cookie.value.len() > caps::COOKIE_VALUE {
                    return Err(bad(&format!(
                        "value of {} bytes > {}",
                        cookie.value.len(),
                        caps::COOKIE_VALUE
                    )));
                }
                if cookie.domain.trim().starts_with('.') {
                    cookie.host_only = false;
                }
                (&cookie.name, &mut cookie.domain, &cookie.path)
            }
            CookieChange::Remove { name, domain, path } => (&*name, domain, &*path),
        };
        if name.is_empty() {
            return Err(bad("no name"));
        }
        if !path.starts_with('/') {
            return Err(bad("path does not start with /"));
        }
        let host = domain.trim().trim_start_matches('.').to_ascii_lowercase();
        if !cookie_host(&host) {
            return Err(bad("domain is a public suffix or empty"));
        }
        *domain = host;
    }
    Ok(())
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
            host_only: true,
            path: "/".into(),
            expires: None,
            secure: true,
            http_only: true,
            same_site: "Lax".into(),
            partitioned: false,
        }
    }

    fn push(changes: Vec<CookieChange>) -> Push {
        Push {
            writer: "tile:iPad".into(),
            epoch: 0,
            changes,
            storage: vec![],
            basis_ms: None,
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
        assert!(diff(std::slice::from_ref(&a), std::slice::from_ref(&a)).is_empty());
    }

    #[test]
    fn groups_by_site() {
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

    /// 209 §4.4: the caps of one push and each cookie's shape are checked before any work.
    #[test]
    fn validate_normalises_and_refuses() {
        let mut ok = push(vec![
            CookieChange::Set {
                cookie: cookie("a", "1", ".App.Example.com"),
            },
            CookieChange::Remove {
                name: "b".into(),
                domain: ".Example.com".into(),
                path: "/".into(),
            },
        ]);
        ok.writer = format!("tile:{}", "d".repeat(200));
        validate(&mut ok).unwrap();
        let CookieChange::Set { cookie: a } = &ok.changes[0] else {
            panic!()
        };
        assert_eq!((a.domain.as_str(), a.host_only), ("app.example.com", false));
        let CookieChange::Remove { domain, .. } = &ok.changes[1] else {
            panic!()
        };
        assert_eq!(domain, "example.com");
        assert_eq!(ok.writer.chars().count(), WRITER_CHARS);

        let refused = |c: Cookie| validate(&mut push(vec![CookieChange::Set { cookie: c }]));
        let mut c = cookie("", "1", "example.com");
        assert!(matches!(refused(c.clone()), Err(ProfileError::Invalid(_))));
        c.name = "n".into();
        c.path = "x".into();
        assert!(refused(c.clone()).is_err());
        c.path = "/".into();
        c.value = "v".repeat(caps::COOKIE_VALUE + 1);
        let e = refused(c.clone()).unwrap_err().to_string();
        assert!(e.contains("change 0") && !e.contains("vvvv"), "{e}");
        c.value = "1".into();
        for suffix in ["co.uk", ".github.io", ""] {
            c.domain = suffix.into();
            assert!(refused(c.clone()).is_err(), "{suffix}");
        }
        for host in ["127.0.0.1", "localhost", "user.github.io"] {
            c.domain = host.into();
            assert!(refused(c.clone()).is_ok(), "{host}");
        }
        let many = (0..=caps::PUSH_CHANGES)
            .map(|i| CookieChange::Set {
                cookie: cookie(&format!("c{i}"), "1", "example.com"),
            })
            .collect();
        assert!(
            validate(&mut push(many))
                .unwrap_err()
                .to_string()
                .contains("2001 changes > 2000")
        );
        let mut snaps = push(vec![]);
        snaps.storage = (0..=caps::PUSH_STORAGE)
            .map(|i| OriginStorage {
                origin: format!("https://s{i}.example.com"),
                local_storage: vec![],
                captured_ms: 1,
            })
            .collect();
        assert!(validate(&mut snaps).is_err());
    }

    #[test]
    fn site_of_ips_single_labels_and_private_suffixes() {
        assert_eq!(site_of("127.0.0.1"), "127.0.0.1");
        assert_eq!(site_of("[::1]"), "::1");
        assert_eq!(site_of("localhost"), "localhost");
        assert_eq!(site_of("app.localhost"), "app.localhost");
        assert_eq!(site_of("user.github.io"), "user.github.io");
        assert_eq!(site_of("a.user.github.io"), "user.github.io");
        assert_eq!(site_of("github.io"), "github.io");
        assert_eq!(site_of(".WWW.Example.co.uk."), "example.co.uk");
    }
}
