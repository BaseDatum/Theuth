//! The shared browser profile inside an agent's obscura session (209 D4, §4.2, §9).
//!
//! [`crate::tools::Ctx`] loads every live profile row into a session's isolate before that
//! session's first `browser_*` call ([`Ctx::ensure_profile`](crate::tools::Ctx)), through
//! obscura's `browser_set_storage_state`: cookies with `host_only` (209 §4.3), then each origin's
//! localStorage. Loading lives in `Ctx`, never in `session::spawn`: the [`crate::Browsers`] actor
//! also serves isolates keyed by private ids for the render provider and scrape slots, and those
//! stay cookie-less (122 §5). A row that changed since it was applied is applied again before the
//! session's next `browser_open` to its site.
//!
//! Saves are per-cookie deltas against what the isolate was given ([`Loaded::baseline`]):
//! `browser_close` checkpoints, and every isolate drop (`Close`, idle `Sweep`, `CloseAll`,
//! `CloseTree`) exports the jar first ([`save_dropped`]). Cookies brought in by
//! `browser_import_cookies` are remembered by key and never saved (209 §4.2, §11 Q2).
//!
//! What a session holds is kept in a process-wide map, like `artifact`'s preview waits: the tools
//! and the actor's drop path share it, and the registry's synchronous
//! [`tools::Tool::credentialed`] reads it (209 D14). Cookie and storage values never reach
//! `tracing`, events or a tool result; replies are scrubbed of loaded values ([`scrub`]).
//!
//! obscura before `theuth/209-host-only` neither exports nor imports `host_only`: the field is
//! sent anyway (ignored there), an export without it keeps the value the profile had for that
//! cookie, and a new cookie counts as host-only only when its name has the `__Host-` prefix.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use parking_lot::{Mutex, RwLock};
use runtime::SessionId;
use serde::Deserialize;
use serde_json::{Value, json};
use webprofile::{Cookie, LoadedProfile, OriginStorage, ProfileError, site_of};

/// Shortest cookie value [`scrub`] replaces (209 §9).
const SCRUB_MIN: usize = 8;

/// A cookie's identity as the profile matches it: `(name, domain without a leading '.', lower
/// case, path)`.
type Key = (String, String, String);

fn key(name: &str, domain: &str, path: &str) -> Key {
    (
        name.to_owned(),
        domain.trim_start_matches('.').to_ascii_lowercase(),
        path.to_owned(),
    )
}

fn key_of(c: &Cookie) -> Key {
    key(&c.name, &c.domain, &c.path)
}

/// What one session's isolate holds from the profile.
#[derive(Clone, Default)]
pub(crate) struct Loaded {
    /// A load succeeded: saves happen and acting calls on profile sites are credentialed.
    active: bool,
    /// The profile epoch saves are made under.
    epoch: u64,
    /// Per site: the row's `updated_ms` last applied.
    rows: HashMap<String, u64>,
    /// The cookies the profile holds for this isolate as far as it knows: loaded, re-applied or
    /// saved. A save is the diff of the isolate's export against it.
    baseline: Vec<Cookie>,
    /// localStorage per origin, as loaded or saved.
    storage: HashMap<String, Vec<(String, String)>>,
    /// Keys of cookies `browser_import_cookies` brought in: never saved.
    imported: HashSet<Key>,
    /// Site of the page the session is on, for [`credentialed_current`].
    current: Option<String>,
}

/// Sessions that went through [`crate::tools::Ctx`]'s load.
static SESSIONS: LazyLock<Mutex<HashMap<SessionId, Loaded>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Sites the profile holds cookies for, as last seen in a load, a `web_profile_sites` listing or
/// a [`webprofile::WebProfileChanged`]. Only grows until restart: forgetting is P2.
static KNOWN: LazyLock<RwLock<HashSet<String>>> = LazyLock::new(|| RwLock::new(HashSet::new()));

/// Serialises first loads so a second concurrent call of a session waits for the first's.
static LOADING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether `session` went through a load (successful or not).
pub(crate) fn seen(session: SessionId) -> bool {
    SESSIONS.lock().contains_key(&session)
}

/// Whether `session` has a profile loaded.
pub(crate) fn active(session: SessionId) -> bool {
    SESSIONS.lock().get(&session).is_some_and(|l| l.active)
}

/// The guard first loads run under.
pub(crate) async fn loading() -> tokio::sync::MutexGuard<'static, ()> {
    LOADING.lock().await
}

/// `session`'s isolate is gone: forget what it held and return it for a last save.
pub(crate) fn take(session: SessionId) -> Option<Loaded> {
    SESSIONS.lock().remove(&session)
}

/// Remember `sites` as holding profile cookies (a [`webprofile::WebProfileChanged`], a listing).
pub(crate) fn know(sites: impl IntoIterator<Item = String>) {
    KNOWN.write().extend(sites);
}

/// 209 D14: whether an acting call on `site` acts with the user's sign-in. A session whose load
/// found no profile (off, unavailable) acts with nothing of it.
fn credentialed_site(session: SessionId, site: &str) -> bool {
    if SESSIONS.lock().get(&session).is_some_and(|l| !l.active) {
        return false;
    }
    KNOWN.read().contains(site)
}

/// [`credentialed_site`] for the site of `url` (`browser_open`'s target).
pub(crate) fn credentialed_url(session: SessionId, url: &str) -> bool {
    site_of_url(url).is_some_and(|site| credentialed_site(session, &site))
}

/// [`credentialed_site`] for the page the session is on (click, fill, type, ...).
pub(crate) fn credentialed_current(session: SessionId) -> bool {
    let current = SESSIONS
        .lock()
        .get(&session)
        .and_then(|l| l.current.clone());
    current.is_some_and(|site| credentialed_site(session, &site))
}

/// Whether the session's current site is worth tracking: it has a profile and the profile holds
/// any site at all.
pub(crate) fn tracking(session: SessionId) -> bool {
    active(session) && !KNOWN.read().is_empty()
}

/// Record the page `session` is on.
pub(crate) fn set_current(session: SessionId, url: &str) {
    if let Some(l) = SESSIONS.lock().get_mut(&session) {
        l.current = site_of_url(url);
    }
}

/// The registrable domain of an http(s) address.
pub(crate) fn site_of_url(url: &str) -> Option<String> {
    let u = url::Url::parse(url.trim()).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = match u.host()? {
        url::Host::Domain(d) => d.to_owned(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    Some(site_of(&host))
}

/// 209 §9: every profile cookie value of [`SCRUB_MIN`] or more characters the session holds is
/// replaced by `[cookie <name>]` in `text`.
pub(crate) fn scrub(session: SessionId, text: &mut String) {
    let sessions = SESSIONS.lock();
    let Some(l) = sessions.get(&session) else {
        return;
    };
    for c in &l.baseline {
        if c.value.len() >= SCRUB_MIN && text.contains(c.value.as_str()) {
            *text = text.replace(c.value.as_str(), &format!("[cookie {}]", c.name));
        }
    }
}

/// `browser_import_cookies` brought these cookies (`name`, `domain`, `path` of each) into the
/// session: never save them. Call after the session's load, which creates its entry.
pub(crate) fn imported(session: SessionId, cookies: &[Value]) {
    let mut sessions = SESSIONS.lock();
    let Some(l) = sessions.get_mut(&session) else {
        return;
    };
    for c in cookies {
        let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or_default();
        let path = c.get("path").and_then(Value::as_str).unwrap_or("/");
        l.imported.insert(key(s("name"), s("domain"), path));
    }
}

/// The calls that put a loaded profile into an isolate: `browser_set_storage_state` arguments,
/// cookies first (one call, so a storage quota error cannot cost the sign-in), then storage.
pub(crate) struct Apply {
    /// Arguments of each `browser_set_storage_state`, in order.
    pub calls: Vec<Value>,
}

/// Record a full load (`load: None` means the profile is off or unavailable) for `session` and
/// return the calls that apply it.
pub(crate) fn loaded(session: SessionId, load: Option<LoadedProfile>) -> Apply {
    let mut sessions = SESSIONS.lock();
    let l = sessions.entry(session).or_default();
    let Some(load) = load else {
        return Apply { calls: Vec::new() };
    };
    l.active = true;
    l.epoch = load.epoch;
    absorb(l, load)
}

/// A row may have changed since `session` applied it: record `load` (the site's row) and return
/// the calls that bring the isolate up to it. Rows not newer than the applied one are skipped.
pub(crate) fn refreshed(session: SessionId, load: LoadedProfile) -> Apply {
    let mut sessions = SESSIONS.lock();
    let Some(l) = sessions.get_mut(&session).filter(|l| l.active) else {
        return Apply { calls: Vec::new() };
    };
    absorb(l, load)
}

/// Fold every newer row of `load` into `l` and build the calls: set each row cookie (imported
/// keys excepted), delete the cookies of the site the row no longer has, write each origin whose
/// snapshot differs.
fn absorb(l: &mut Loaded, load: LoadedProfile) -> Apply {
    let mut set = Vec::new();
    let mut origins = Vec::new();
    for row in load.sites {
        let site = row.jar.site.clone();
        if !row.jar.cookies.is_empty() {
            KNOWN.write().insert(site.clone());
        }
        if l.rows.get(&site).is_some_and(|at| *at >= row.updated_ms) {
            continue;
        }
        l.rows.insert(site.clone(), row.updated_ms);
        let keep: HashSet<Key> = row.jar.cookies.iter().map(key_of).collect();
        let mut gone = Vec::new();
        l.baseline.retain(|c| {
            let of_site = site_of(&c.domain) == site;
            if of_site && !keep.contains(&key_of(c)) && !l.imported.contains(&key_of(c)) {
                gone.push(c.clone());
            }
            !of_site
        });
        set.extend(gone.iter().map(removal_json));
        for c in row.jar.cookies {
            if !l.imported.contains(&key_of(&c)) {
                set.push(cookie_json(&c));
            }
            l.baseline.push(c);
        }
        for o in row.jar.origins {
            if canonical_origin(&o.origin) && l.storage.get(&o.origin) != Some(&o.local_storage) {
                origins.push(json!({ "origin": o.origin, "localStorage": o.local_storage }));
                l.storage.insert(o.origin, o.local_storage);
            }
        }
    }
    let mut calls = Vec::new();
    if !set.is_empty() {
        calls.push(json!({ "state": { "cookies": set } }));
    }
    if !origins.is_empty() {
        calls.push(json!({ "state": { "origins": origins } }));
    }
    Apply { calls }
}

/// obscura's import shape of one cookie, with `host_only` (209 §4.3).
fn cookie_json(c: &Cookie) -> Value {
    json!({
        "name": c.name,
        "value": c.value,
        "domain": c.domain,
        "host_only": c.host_only,
        "path": c.path,
        "expires": c.expires,
        "secure": c.secure,
        "http_only": c.http_only,
        "same_site": c.same_site,
    })
}

/// A delete through obscura's import: `expires: 0` removes the `(name, domain, path)` entry.
fn removal_json(c: &Cookie) -> Value {
    let mut v = cookie_json(c);
    v["value"] = json!("");
    v["expires"] = json!(0);
    v
}

/// What obscura's `browser_set_storage_state` accepts as an origin: canonical http(s), no path.
fn canonical_origin(origin: &str) -> bool {
    url::Url::parse(origin).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https") && u.origin().ascii_serialization() == origin
    })
}

/// One cookie of obscura's `browser_storage_state` export. No `Debug`: it holds the value.
#[derive(Deserialize)]
struct Exported {
    name: String,
    value: String,
    domain: String,
    #[serde(default = "root")]
    path: String,
    #[serde(default)]
    secure: bool,
    #[serde(default)]
    http_only: bool,
    #[serde(default)]
    same_site: String,
    #[serde(default)]
    expires: Option<i64>,
    /// Absent before obscura `theuth/209-host-only`.
    #[serde(default)]
    host_only: Option<bool>,
}

fn root() -> String {
    "/".to_owned()
}

/// One origin of the export.
#[derive(Deserialize)]
struct ExportedOrigin {
    origin: String,
    #[serde(default, rename = "localStorage")]
    local_storage: Vec<(String, String)>,
}

/// The export as a whole.
#[derive(Deserialize)]
struct Export {
    #[serde(default)]
    cookies: Vec<Exported>,
    #[serde(default)]
    origins: Vec<ExportedOrigin>,
}

/// What one save sends and what the session's state becomes once it is applied.
pub(crate) struct Save {
    epoch: u64,
    changes: Vec<webprofile::CookieChange>,
    storage: Vec<OriginStorage>,
    after: Vec<Cookie>,
    after_storage: HashMap<String, Vec<(String, String)>>,
}

impl Save {
    /// Nothing changed.
    pub(crate) fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.storage.is_empty()
    }
}

/// The delta of `export` (the text obscura's `browser_storage_state` returns) against `l`
/// (209 §4.2), imported cookies left out on both sides. `None` when the text is not an export.
pub(crate) fn plan(l: &Loaded, export: &str, now_ms: u64) -> Option<Save> {
    let export: Export = serde_json::from_str(export).ok()?;
    let had: HashMap<Key, &Cookie> = l.baseline.iter().map(|c| (key_of(c), c)).collect();
    let after: Vec<Cookie> = export
        .cookies
        .into_iter()
        .filter(|c| !l.imported.contains(&key(&c.name, &c.domain, &c.path)))
        .map(|c| {
            let before = had.get(&key(&c.name, &c.domain, &c.path));
            Cookie {
                host_only: c.host_only.unwrap_or_else(|| {
                    before.map_or(c.name.starts_with("__Host-"), |b| b.host_only)
                }),
                partitioned: before.is_some_and(|b| b.partitioned),
                name: c.name,
                value: c.value,
                // The profile's spelling (`.example.com` from WebKit), so an unchanged cookie
                // is no change.
                domain: before.map_or(c.domain, |b| b.domain.clone()),
                path: c.path,
                expires: c.expires,
                secure: c.secure,
                http_only: c.http_only,
                same_site: c.same_site,
            }
        })
        .collect();
    let before: Vec<Cookie> = l
        .baseline
        .iter()
        .filter(|c| !l.imported.contains(&key_of(c)))
        .cloned()
        .collect();
    let changes = webprofile::merge::diff(&before, &after);
    let mut storage = Vec::new();
    let mut after_storage = l.storage.clone();
    for o in export.origins {
        if o.local_storage.is_empty() || l.storage.get(&o.origin) == Some(&o.local_storage) {
            continue;
        }
        storage.push(OriginStorage {
            origin: o.origin.clone(),
            local_storage: o.local_storage.clone(),
            captured_ms: now_ms,
        });
        after_storage.insert(o.origin, o.local_storage);
    }
    Some(Save {
        epoch: l.epoch,
        changes,
        storage,
        after,
        after_storage,
    })
}

/// A copy of `session`'s state for a checkpoint save, when it has a profile.
pub(crate) fn snapshot(session: SessionId) -> Option<Loaded> {
    SESSIONS.lock().get(&session).filter(|l| l.active).cloned()
}

/// `save` reached the profile under `epoch`: it is the session's new baseline.
fn saved(session: SessionId, save: Save, epoch: u64) {
    if let Some(l) = SESSIONS.lock().get_mut(&session) {
        l.baseline = save.after;
        l.storage = save.after_storage;
        l.epoch = epoch;
    }
}

/// Push `save` as this engine's obscura (writer `obscura:<engine>`). Logs counts and sites, never
/// values. `Some(epoch)` when it was written.
pub(crate) async fn push(session: SessionId, save: &Save) -> Option<u64> {
    let profile = webprofile::Handle::lookup()?;
    let (engine, _) = agent::communication::mesh::engine_names().await;
    let msg = webprofile::Push {
        writer: format!("{}{engine}", webprofile::WRITER_OBSCURA),
        epoch: save.epoch,
        changes: save.changes.clone(),
        storage: save.storage.clone(),
    };
    match profile.push(msg).await {
        Ok(r) => {
            tracing::debug!(%session, sites = ?r.sites, applied = r.applied, "browser profile saved");
            Some(r.epoch)
        }
        Err(ProfileError::StaleEpoch { current }) => {
            tracing::info!(%session, current, "browser profile save refused: the profile was cleared since this session loaded it");
            None
        }
        Err(e) => {
            tracing::info!(%session, "browser profile save skipped: {e}");
            None
        }
    }
}

/// Checkpoint: diff `export` against the session's state, push, and make it the new baseline.
pub(crate) async fn checkpoint(session: SessionId, export: &str) {
    let Some(l) = snapshot(session) else { return };
    let Some(save) = plan(&l, export, webprofile::store::now_ms()) else {
        tracing::info!(%session, "browser profile save skipped: unreadable export");
        return;
    };
    if save.is_empty() {
        return;
    }
    if let Some(epoch) = push(session, &save).await {
        saved(session, save, epoch);
    }
}

/// Last save of a dropped isolate (209 D4): export its jar through the handle the actor just
/// removed (the thread ends once this call and any in flight are done), diff, push.
pub(crate) async fn save_dropped(session: SessionId, handle: crate::session::Handle, l: Loaded) {
    if !l.active {
        return;
    }
    let export = match handle.call("browser_storage_state", json!({})).await {
        Ok(v) => v,
        Err(e) => {
            tracing::info!(%session, "browser profile save skipped: {e}");
            return;
        }
    };
    drop(handle);
    let (text, is_error) = crate::tools::text_of(&export);
    if is_error {
        tracing::info!(%session, "browser profile save skipped: the export failed");
        return;
    }
    let Some(save) = plan(&l, &text, webprofile::store::now_ms()) else {
        tracing::info!(%session, "browser profile save skipped: unreadable export");
        return;
    };
    if !save.is_empty() {
        push(session, &save).await;
    }
}

/// Fill [`KNOWN`] from the profile's site list once it answers (plugin start), so the first
/// `browser_open` of a session to a signed-in site is already credentialed (209 D14).
pub(crate) async fn learn_sites() {
    for _ in 0..30 {
        if let Some(profile) = webprofile::Handle::lookup() {
            match profile.sites().await {
                Ok(sites) => {
                    know(sites.into_iter().filter(|s| s.cookies > 0).map(|s| s.site));
                    return;
                }
                Err(ProfileError::Unavailable(why)) if why.contains("off") => return,
                Err(_) => {}
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webprofile::{LoadedSite, SiteJar};

    fn cookie(name: &str, value: &str, domain: &str, host_only: bool) -> Cookie {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            host_only,
            path: "/".into(),
            expires: Some(4_000_000_000),
            secure: true,
            http_only: true,
            same_site: "Lax".into(),
            partitioned: false,
        }
    }

    fn row(site: &str, updated_ms: u64, cookies: Vec<Cookie>) -> LoadedSite {
        LoadedSite {
            jar: SiteJar {
                cookies,
                ..SiteJar::new(site)
            },
            updated_ms,
            writer: "tile:ipad".into(),
        }
    }

    fn export(cookies: Value) -> String {
        json!({ "cookies": cookies, "origins": [] }).to_string()
    }

    /// The old obscura's export has no `host_only`: the loaded value is kept, so an unchanged
    /// `__Host-` cookie is no change; an import is never saved.
    #[test]
    fn plan_keeps_host_only_and_skips_imported() {
        let session = SessionId::new();
        let sid = cookie("__Host-sid", "s3cr3t-value", "app.example.com", true);
        let apply = loaded(
            session,
            Some(LoadedProfile {
                epoch: 2,
                sites: vec![row("example.com", 10, vec![sid.clone()])],
            }),
        );
        assert_eq!(apply.calls.len(), 1);
        assert_eq!(apply.calls[0]["state"]["cookies"][0]["host_only"], true);
        imported(
            session,
            &[json!({ "name": "gh", "domain": ".github.com", "path": "/" })],
        );
        let l = snapshot(session).unwrap();
        let text = export(json!([
            { "name": "__Host-sid", "value": "s3cr3t-value", "domain": "app.example.com", "path": "/",
              "secure": true, "http_only": true, "same_site": "Lax", "expires": 4_000_000_000i64 },
            { "name": "gh", "value": "imported-value", "domain": "github.com", "path": "/" },
        ]));
        let save = plan(&l, &text, 1).unwrap();
        assert!(save.is_empty(), "{:?}", save.changes);
        assert_eq!(save.epoch, 2);

        // A refreshed value is one `set` carrying the profile's host_only.
        let text = export(json!([
            { "name": "__Host-sid", "value": "rotated-value", "domain": "app.example.com", "path": "/",
              "secure": true, "http_only": true, "same_site": "Lax", "expires": 4_000_000_000i64 },
        ]));
        let save = plan(&l, &text, 1).unwrap();
        match save.changes.as_slice() {
            [webprofile::CookieChange::Set { cookie }] => {
                assert!(cookie.host_only);
                assert_eq!(cookie.value, "rotated-value");
            }
            other => panic!("{other:?}"),
        }
        take(session);
    }

    /// A newer row is applied again, a row not newer is not; cookies the row dropped are deleted.
    #[test]
    fn refresh_applies_only_newer_rows() {
        let session = SessionId::new();
        let a = cookie("a", "aaaaaaaaaa", "example.com", false);
        let b = cookie("b", "bbbbbbbbbb", "example.com", false);
        loaded(
            session,
            Some(LoadedProfile {
                epoch: 0,
                sites: vec![row("example.com", 10, vec![a.clone(), b])],
            }),
        );
        let same = refreshed(
            session,
            LoadedProfile {
                epoch: 0,
                sites: vec![row("example.com", 10, vec![a.clone()])],
            },
        );
        assert!(same.calls.is_empty());
        let newer = refreshed(
            session,
            LoadedProfile {
                epoch: 0,
                sites: vec![row("example.com", 11, vec![a])],
            },
        );
        let cookies = newer.calls[0]["state"]["cookies"].as_array().unwrap();
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0]["name"], "b");
        assert_eq!(cookies[0]["expires"], 0);
        assert_eq!(cookies[1]["name"], "a");
        take(session);
    }

    #[test]
    fn scrub_replaces_long_values_only() {
        let session = SessionId::new();
        loaded(
            session,
            Some(LoadedProfile {
                epoch: 0,
                sites: vec![row(
                    "example.com",
                    1,
                    vec![
                        cookie("sid", "0123456789abcdef", "example.com", false),
                        cookie("short", "1234567", "example.com", false),
                    ],
                )],
            }),
        );
        let mut text = "token=0123456789abcdef; n=1234567".to_owned();
        scrub(session, &mut text);
        assert_eq!(text, "token=[cookie sid]; n=1234567");
        take(session);
    }

    #[test]
    fn credentialed_follows_known_sites_and_inactive_sessions() {
        let (on, off) = (SessionId::new(), SessionId::new());
        know(["credentialed-test.example".to_owned()]);
        loaded(on, Some(LoadedProfile::default()));
        loaded(off, None);
        assert!(credentialed_url(
            on,
            "https://app.credentialed-test.example/x"
        ));
        assert!(!credentialed_url(on, "https://other.example/"));
        assert!(!credentialed_url(
            off,
            "https://app.credentialed-test.example/x"
        ));
        assert!(!credentialed_current(on));
        set_current(on, "https://www.credentialed-test.example/");
        assert!(credentialed_current(on));
        take(on);
        take(off);
    }
}
