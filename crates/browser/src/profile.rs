//! The shared browser profile inside an agent's obscura session (209 D4, §4.2, §9).
//!
//! [`crate::tools::Ctx`] loads every live profile row into a session's isolate before that
//! session's first `browser_*` call ([`Ctx::ensure_profile`](crate::tools::Ctx)), through
//! obscura's `browser_set_storage_state`: cookies with `host_only` (209 §4.3), then each origin's
//! localStorage. Loading lives in `Ctx`, never in `session::spawn`: the [`crate::Browsers`] actor
//! also serves isolates keyed by private ids for the render provider and scrape slots, and those
//! stay cookie-less (122 §5). A load that fails (store starting, vault not ready) records nothing
//! and is retried by the session's next call after a backoff ([`retry_due`]); loads of one session
//! are serialised by that session's own guard ([`guard`]), never a process-wide one. A site whose
//! rows changed since they were applied is applied again before the session's next
//! `browser_open` to it.
//!
//! Saves are per-cookie deltas against what the isolate was given ([`Loaded::baseline`]), and a
//! session only updates sites the profile already holds: a site first signed into in obscura
//! stays in that session (new sites enter through the user's web view, 209 §4.2). `browser_close`
//! checkpoints, and every isolate drop (`Close`, idle `Sweep`, `CloseAll`, `CloseTree`) exports
//! the jar first, bounded ([`save_dropped`]). A save refused as `stale-epoch` (sign out
//! everywhere happened) clears the isolate's profile cookies and loads again ([`reset`]).
//! Cookies brought in by `browser_import_cookies` are remembered and never saved (209 §4.2, §11
//! Q2).
//!
//! What a session holds is kept in a process-wide map, like `artifact`'s preview waits: the tools
//! and the actor's drop path share it, and the registry's synchronous
//! [`tools::Tool::credentialed`] reads it (209 D14): a site is signed in for a session when its
//! load gave the session cookies of it, or when the profile is known to hold it ([`know`], kept
//! current by `WebProfileChanged`, which the profile also publishes for rows replicated from
//! another engine). Cookie and storage values never reach `tracing`, events or a tool result;
//! replies are scrubbed of loaded and imported values ([`scrub`]).
//!
//! obscura before `theuth/209-host-only` neither exports nor imports `host_only`: the field is
//! sent anyway (ignored there), an export without it keeps the value the profile had for that
//! cookie, and a new cookie counts as host-only only when its name has the `__Host-` prefix.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use runtime::SessionId;
use serde::Deserialize;
use serde_json::{Value, json};
use webprofile::{Cookie, LoadedProfile, OriginStorage, ProfileError, site_of};

/// Shortest cookie value [`scrub`] replaces (209 §9).
const SCRUB_MIN: usize = 8;

/// How long the last save of a dropped isolate waits for its export (209 D4): the close is not
/// held up longer than this by the profile.
pub(crate) const DROP_EXPORT_WAIT: Duration = Duration::from_secs(2);

/// First and longest wait before a failed load is tried again.
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// A cookie's identity as the profile matches it: `(name, domain without a leading '.', lower
/// case, path)`.
type Key = (String, String, String);

fn key(name: &str, domain: &str, path: &str) -> Key {
    webprofile::merge::norm(name, domain, path)
}

fn key_of(c: &Cookie) -> Key {
    key(&c.name, &c.domain, &c.path)
}

/// What one session's isolate holds from the profile.
#[derive(Clone, Default)]
pub(crate) struct Loaded {
    /// A load was applied: saves happen and acting calls on its sites are credentialed.
    active: bool,
    /// The profile epoch saves are made under.
    epoch: u64,
    /// Per site: the newest row `updated_ms` applied.
    rows: HashMap<String, u64>,
    /// The cookies the profile holds for this isolate as far as it knows: loaded, re-applied or
    /// saved. A save is the diff of the isolate's export against it.
    baseline: Vec<Cookie>,
    /// localStorage per origin, as loaded or saved.
    storage: HashMap<String, Vec<(String, String)>>,
    /// Cookies `browser_import_cookies` brought in, by key, with their values for [`scrub`]:
    /// never saved.
    imported: HashMap<Key, String>,
    /// Site of the page the session is on, for [`credentialed_current`].
    current: Option<String>,
}

impl Loaded {
    /// Whether the session was given profile cookies of `site`.
    fn has_site(&self, site: &str) -> bool {
        self.rows.contains_key(site) || self.baseline.iter().any(|c| site_of(&c.domain) == site)
    }
}

/// Sessions that went through [`crate::tools::Ctx`]'s load (or imported cookies before one).
static SESSIONS: LazyLock<Mutex<HashMap<SessionId, Loaded>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Sites the profile holds, as last seen in a load, a `web_profile_sites` listing or a
/// [`webprofile::WebProfileChanged`] (local or replicated). Only grows until restart: forgetting
/// is P2.
static KNOWN: LazyLock<RwLock<HashSet<String>>> = LazyLock::new(|| RwLock::new(HashSet::new()));

/// Per-session load guards: a second concurrent call of a session waits for the first's load,
/// other sessions do not.
static GUARDS: LazyLock<Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Sessions whose last load failed: when to try again and how many tries failed.
static RETRY: LazyLock<Mutex<HashMap<SessionId, (Instant, u32)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether `session` has a profile applied.
pub(crate) fn active(session: SessionId) -> bool {
    SESSIONS.lock().get(&session).is_some_and(|l| l.active)
}

/// `session`'s load guard.
pub(crate) fn guard(session: SessionId) -> Arc<tokio::sync::Mutex<()>> {
    GUARDS.lock().entry(session).or_default().clone()
}

/// Whether a load of `session` may be tried now: never failed, or its backoff ran out.
pub(crate) fn retry_due(session: SessionId) -> bool {
    RETRY
        .lock()
        .get(&session)
        .is_none_or(|(at, _)| Instant::now() >= *at)
}

/// A load of `session` failed: the next one waits 1 s, then twice as long each time, at most 30 s.
pub(crate) fn failed(session: SessionId) {
    let mut retry = RETRY.lock();
    let tries = retry.get(&session).map_or(0, |(_, n)| *n);
    let wait = RETRY_FIRST.saturating_mul(1 << tries.min(5)).min(RETRY_MAX);
    retry.insert(session, (Instant::now() + wait, tries + 1));
}

/// `session`'s isolate is gone: forget what it held and return it for a last save.
pub(crate) fn take(session: SessionId) -> Option<Loaded> {
    GUARDS.lock().remove(&session);
    RETRY.lock().remove(&session);
    SESSIONS.lock().remove(&session)
}

/// Remember `sites` as held by the profile (a [`webprofile::WebProfileChanged`], a listing).
pub(crate) fn know(sites: impl IntoIterator<Item = String>) {
    KNOWN.write().extend(sites);
}

/// 209 D14: whether an acting call on `site` acts with the user's sign-in: the session's load gave
/// it cookies of `site`, or the profile is known to hold `site` (a session that has not loaded
/// yet loads before acting).
fn credentialed_site(session: SessionId, site: &str) -> bool {
    KNOWN.read().contains(site)
        || SESSIONS
            .lock()
            .get(&session)
            .is_some_and(|l| l.active && l.has_site(site))
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

/// `browser_eval`: credentialed on a signed-in page, and on any page of a session that holds
/// profile cookies (a script can `fetch` another signed-in origin, 209 D14).
pub(crate) fn credentialed_eval(session: SessionId) -> bool {
    credentialed_current(session)
        || SESSIONS
            .lock()
            .get(&session)
            .is_some_and(|l| l.active && !l.baseline.is_empty())
}

/// Whether the page a session is on is worth tracking: it has a profile and the profile holds any
/// site at all.
pub(crate) fn tracking(session: SessionId) -> bool {
    SESSIONS.lock().get(&session).is_some_and(|l| {
        l.active && (!l.baseline.is_empty() || !l.rows.is_empty() || !KNOWN.read().is_empty())
    })
}

/// Record the page `session` is on. Returns the site when the session just moved onto a
/// signed-in site from a page that was not one: that navigation itself was not gated (209 §4.6),
/// the caller records it.
pub(crate) fn set_current(session: SessionId, url: &str) -> Option<String> {
    let site = site_of_url(url);
    let before = {
        let mut sessions = SESSIONS.lock();
        let l = sessions.get_mut(&session)?;
        std::mem::replace(&mut l.current, site.clone())
    };
    let site = site?;
    let was = before.is_some_and(|b| credentialed_site(session, &b));
    (!was && credentialed_site(session, &site)).then_some(site)
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

/// 209 §9: every profile or imported cookie value of [`SCRUB_MIN`] or more characters the session
/// holds is replaced by `[cookie <name>]` in `text`.
pub(crate) fn scrub(session: SessionId, text: &mut String) {
    let sessions = SESSIONS.lock();
    let Some(l) = sessions.get(&session) else {
        return;
    };
    let values = l
        .baseline
        .iter()
        .map(|c| (c.name.as_str(), c.value.as_str()))
        .chain(
            l.imported
                .iter()
                .map(|((n, ..), v)| (n.as_str(), v.as_str())),
        );
    for (name, value) in values {
        if value.len() >= SCRUB_MIN && text.contains(value) {
            *text = text.replace(value, &format!("[cookie {name}]"));
        }
    }
}

/// `browser_import_cookies` brought these cookies (`name`, `domain`, `path`, `value` of each) into
/// the session: never save them, and scrub their values from replies.
pub(crate) fn imported(session: SessionId, cookies: &[Value]) {
    let mut sessions = SESSIONS.lock();
    let l = sessions.entry(session).or_default();
    for c in cookies {
        let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or_default();
        let path = c.get("path").and_then(Value::as_str).unwrap_or("/");
        l.imported
            .insert(key(s("name"), s("domain"), path), s("value").to_owned());
    }
}

/// The calls that put a loaded profile into an isolate: `browser_set_storage_state` arguments,
/// cookies first (one call, so a storage quota error cannot cost the sign-in), then storage.
pub(crate) struct Apply {
    /// Arguments of each `browser_set_storage_state`, in order.
    pub calls: Vec<Value>,
}

/// Record a full load for `session` and return the calls that apply it; the session counts as
/// loaded once they ran ([`activate`]).
pub(crate) fn loaded(session: SessionId, load: LoadedProfile) -> Apply {
    RETRY.lock().remove(&session);
    let mut sessions = SESSIONS.lock();
    let l = sessions.entry(session).or_default();
    l.epoch = load.epoch;
    absorb(l, load)
}

/// The calls of [`loaded`] ran: saves happen from now on.
pub(crate) fn activate(session: SessionId) {
    if let Some(l) = SESSIONS.lock().get_mut(&session) {
        l.active = true;
    }
}

/// Rows may have changed since `session` applied them: record `load` and return the calls that
/// bring the isolate up to it. Sites not newer than the applied ones are skipped.
pub(crate) fn refreshed(session: SessionId, load: LoadedProfile) -> Apply {
    let mut sessions = SESSIONS.lock();
    let Some(l) = sessions.get_mut(&session).filter(|l| l.active) else {
        return Apply { calls: Vec::new() };
    };
    absorb(l, load)
}

/// A save was refused as `stale-epoch` (209 §6: sign out everywhere happened elsewhere): forget
/// what the session was given and return the calls that delete those cookies from the isolate.
/// The session loads again at the new epoch.
pub(crate) fn reset(session: SessionId) -> Apply {
    let mut sessions = SESSIONS.lock();
    let Some(l) = sessions.get_mut(&session) else {
        return Apply { calls: Vec::new() };
    };
    let gone: Vec<Value> = l
        .baseline
        .iter()
        .filter(|c| !l.imported.contains_key(&key_of(c)))
        .map(removal_json)
        .collect();
    *l = Loaded {
        imported: std::mem::take(&mut l.imported),
        current: l.current.take(),
        ..Loaded::default()
    };
    let calls = if gone.is_empty() {
        Vec::new()
    } else {
        vec![json!({ "state": { "cookies": gone } })]
    };
    Apply { calls }
}

/// Fold every newer site of `load` into `l` and build the calls: set each row cookie (imported
/// keys excepted), delete the cookies of the site the profile no longer has, write each origin
/// whose snapshot differs.
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
            if of_site && !keep.contains(&key_of(c)) && !l.imported.contains_key(&key_of(c)) {
                gone.push(c.clone());
            }
            !of_site
        });
        set.extend(gone.iter().map(removal_json));
        for c in row.jar.cookies {
            if !l.imported.contains_key(&key_of(&c)) {
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

/// Whether a save of `l` may write `site`: the profile already holds it (the session loaded it,
/// or it is known). A session never adds a site (209 §4.2).
fn held(l: &Loaded, site: &str) -> bool {
    l.rows.contains_key(site) || KNOWN.read().contains(site)
}

/// The delta of `export` (the text obscura's `browser_storage_state` returns) against `l`
/// (209 §4.2): imported cookies left out on both sides, and only sites the profile holds. An
/// emptied origin is a capture. `None` when the text is not an export.
pub(crate) fn plan(l: &Loaded, export: &str, now_ms: u64) -> Option<Save> {
    let export: Export = serde_json::from_str(export).ok()?;
    let had: HashMap<Key, &Cookie> = l.baseline.iter().map(|c| (key_of(c), c)).collect();
    let after: Vec<Cookie> = export
        .cookies
        .into_iter()
        .filter(|c| {
            let k = key(&c.name, &c.domain, &c.path);
            !l.imported.contains_key(&k) && (had.contains_key(&k) || held(l, &site_of(&c.domain)))
        })
        .map(|c| {
            let before = had.get(&key(&c.name, &c.domain, &c.path));
            Cookie {
                host_only: c.host_only.unwrap_or_else(|| {
                    before.map_or(c.name.starts_with("__Host-"), |b| b.host_only)
                }),
                partitioned: before.is_some_and(|b| b.partitioned),
                name: c.name,
                value: c.value,
                // The profile's spelling, so an unchanged cookie is no change.
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
        .filter(|c| !l.imported.contains_key(&key_of(c)))
        .cloned()
        .collect();
    let changes = webprofile::merge::diff(&before, &after);
    let mut storage = Vec::new();
    let mut after_storage = l.storage.clone();
    for o in export.origins {
        let site = webprofile::merge::origin_host(&o.origin).map(|h| site_of(&h));
        if l.storage.get(&o.origin) == Some(&o.local_storage)
            || (o.local_storage.is_empty() && !l.storage.contains_key(&o.origin))
            || !site.is_some_and(|s| held(l, &s))
        {
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

/// How a save ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Saved {
    /// Written, or nothing to write.
    Done,
    /// Refused as `stale-epoch`: the profile was cleared since the session loaded it.
    Stale,
    /// Not written (no profile, off, unreadable export, a failure); logged.
    Skipped,
}

/// Push `save` as this engine's obscura (writer `obscura:<engine>`). Logs counts and sites, never
/// values. `Ok(epoch)` when it was written.
async fn push(session: SessionId, save: &Save) -> Result<u64, Saved> {
    let profile = webprofile::Handle::lookup().ok_or(Saved::Skipped)?;
    let (engine, _) = agent::communication::mesh::engine_names().await;
    let msg = webprofile::Push {
        writer: format!("{}{engine}", webprofile::WRITER_OBSCURA),
        epoch: save.epoch,
        changes: save.changes.clone(),
        storage: save.storage.clone(),
    };
    match profile.push(msg).await {
        Ok(r) if r.profile == webprofile::ProfileMode::Off => Err(Saved::Skipped),
        Ok(r) => {
            tracing::debug!(%session, sites = ?r.sites, applied = r.applied, "browser profile saved");
            Ok(r.epoch)
        }
        Err(ProfileError::StaleEpoch { current }) => {
            tracing::info!(%session, current, "browser profile save refused: the profile was cleared since this session loaded it");
            Err(Saved::Stale)
        }
        Err(e) => {
            tracing::info!(%session, "browser profile save skipped: {e}");
            Err(Saved::Skipped)
        }
    }
}

/// Checkpoint: diff `export` against the session's state, push, and make it the new baseline.
pub(crate) async fn checkpoint(session: SessionId, export: &str) -> Saved {
    let Some(l) = snapshot(session) else {
        return Saved::Skipped;
    };
    let Some(save) = plan(&l, export, webprofile::store::now_ms()) else {
        tracing::info!(%session, "browser profile save skipped: unreadable export");
        return Saved::Skipped;
    };
    if save.is_empty() {
        return Saved::Done;
    }
    match push(session, &save).await {
        Ok(epoch) => {
            saved(session, save, epoch);
            Saved::Done
        }
        Err(s) => s,
    }
}

/// Last save of a dropped isolate (209 D4): export its jar through the handle the actor just
/// removed, waiting at most [`DROP_EXPORT_WAIT`], then let the handle go (the thread ends once the
/// export and any call in flight are done, so a close is not held up by the save) and push.
pub(crate) async fn save_dropped(session: SessionId, handle: crate::session::Handle, l: Loaded) {
    if !l.active {
        return;
    }
    let export = tokio::time::timeout(
        DROP_EXPORT_WAIT,
        handle.call("browser_storage_state", json!({})),
    )
    .await;
    drop(handle);
    let export = match export {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::info!(%session, "browser profile save skipped: {e}");
            return;
        }
        Err(_) => {
            tracing::info!(%session, "browser profile save skipped: the export took over 2 s");
            return;
        }
    };
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
        let _ = push(session, &save).await;
    }
}

/// Fill [`KNOWN`] from the profile's site list once it answers (plugin start), so the first
/// `browser_open` of a session to a signed-in site is already credentialed (209 D14). Later
/// changes, local or replicated, arrive as [`webprofile::WebProfileChanged`].
pub(crate) async fn learn_sites() {
    for _ in 0..30 {
        if let Some(profile) = webprofile::Handle::lookup()
            && let Ok(sites) = profile.sites().await
        {
            know(sites.into_iter().filter(|s| s.cookies > 0).map(|s| s.site));
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
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

    fn load(session: SessionId, epoch: u64, sites: Vec<LoadedSite>) -> Apply {
        let apply = loaded(session, LoadedProfile { epoch, sites });
        activate(session);
        apply
    }

    fn export(cookies: Value, origins: Value) -> String {
        json!({ "cookies": cookies, "origins": origins }).to_string()
    }

    /// The old obscura's export has no `host_only`: the loaded value is kept, so an unchanged
    /// `__Host-` cookie is no change; an import is never saved and its value is scrubbed.
    #[test]
    fn plan_keeps_host_only_and_skips_imported() {
        let session = SessionId::new();
        let sid = cookie("__Host-sid", "s3cr3t-value", "app.plan-test.example", true);
        let apply = load(session, 2, vec![row("plan-test.example", 10, vec![sid])]);
        assert_eq!(apply.calls.len(), 1);
        assert_eq!(apply.calls[0]["state"]["cookies"][0]["host_only"], true);
        imported(
            session,
            &[
                json!({ "name": "gh", "value": "imported-value", "domain": ".plan-test.example", "path": "/" }),
            ],
        );
        let l = snapshot(session).unwrap();
        let text = export(
            json!([
                { "name": "__Host-sid", "value": "s3cr3t-value", "domain": "app.plan-test.example", "path": "/",
                  "secure": true, "http_only": true, "same_site": "Lax", "expires": 4_000_000_000i64 },
                { "name": "gh", "value": "imported-value", "domain": "plan-test.example", "path": "/" },
            ]),
            json!([]),
        );
        let save = plan(&l, &text, 1).unwrap();
        assert!(save.is_empty(), "{:?}", save.changes);
        assert_eq!(save.epoch, 2);
        let mut reply = "the page says imported-value".to_owned();
        scrub(session, &mut reply);
        assert_eq!(reply, "the page says [cookie gh]");

        // A refreshed value is one `set` carrying the profile's host_only.
        let text = export(
            json!([
                { "name": "__Host-sid", "value": "rotated-value", "domain": "app.plan-test.example", "path": "/",
                  "secure": true, "http_only": true, "same_site": "Lax", "expires": 4_000_000_000i64 },
            ]),
            json!([]),
        );
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

    /// 209 §4.2: a session updates the sites the profile holds and never adds one; an origin it
    /// emptied is a capture.
    #[test]
    fn plan_never_adds_a_site_and_captures_an_emptied_origin() {
        let session = SessionId::new();
        load(
            session,
            0,
            vec![row(
                "held-test.example",
                1,
                vec![cookie("a", "1", "held-test.example", false)],
            )],
        );
        if let Some(l) = SESSIONS.lock().get_mut(&session) {
            l.storage.insert(
                "https://held-test.example".into(),
                vec![("k".into(), "v".into())],
            );
        }
        let l = snapshot(session).unwrap();
        let text = export(
            json!([
                { "name": "a", "value": "1", "domain": "held-test.example", "path": "/",
                  "secure": true, "http_only": true, "same_site": "Lax", "expires": 4_000_000_000i64 },
                { "name": "new", "value": "2", "domain": "www.held-test.example", "path": "/" },
                { "name": "t", "value": "3", "domain": "tracker-never-held.example", "path": "/" },
            ]),
            json!([
                { "origin": "https://held-test.example", "localStorage": [] },
                { "origin": "https://tracker-never-held.example", "localStorage": [["k", "v"]] },
            ]),
        );
        let save = plan(&l, &text, 5).unwrap();
        match save.changes.as_slice() {
            [webprofile::CookieChange::Set { cookie }] => assert_eq!(cookie.name, "new"),
            other => panic!("{other:?}"),
        }
        assert_eq!(save.storage.len(), 1);
        assert_eq!(save.storage[0].origin, "https://held-test.example");
        assert!(save.storage[0].local_storage.is_empty());
        take(session);
    }

    /// A newer row is applied again, a row not newer is not; cookies the row dropped are deleted.
    #[test]
    fn refresh_applies_only_newer_rows() {
        let session = SessionId::new();
        let a = cookie("a", "aaaaaaaaaa", "example.com", false);
        let b = cookie("b", "bbbbbbbbbb", "example.com", false);
        load(session, 0, vec![row("example.com", 10, vec![a.clone(), b])]);
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

    /// 209 §6: a stale save drops what the session was given and deletes it from the isolate.
    #[test]
    fn reset_deletes_loaded_cookies_and_keeps_imports() {
        let session = SessionId::new();
        load(
            session,
            0,
            vec![row(
                "reset-test.example",
                1,
                vec![cookie("sid", "0123456789", "reset-test.example", false)],
            )],
        );
        imported(
            session,
            &[
                json!({ "name": "imp", "value": "imported-0123", "domain": "x.example", "path": "/" }),
            ],
        );
        let apply = reset(session);
        let gone = apply.calls[0]["state"]["cookies"].as_array().unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(
            (gone[0]["name"].as_str(), gone[0]["expires"].as_i64()),
            (Some("sid"), Some(0))
        );
        assert!(!active(session), "loads again at the new epoch");
        let mut text = "imported-0123".to_owned();
        scrub(session, &mut text);
        assert_eq!(text, "[cookie imp]");
        take(session);
    }

    #[test]
    fn scrub_replaces_long_values_only() {
        let session = SessionId::new();
        load(
            session,
            0,
            vec![row(
                "example.com",
                1,
                vec![
                    cookie("sid", "0123456789abcdef", "example.com", false),
                    cookie("short", "1234567", "example.com", false),
                ],
            )],
        );
        let mut text = "token=0123456789abcdef; n=1234567".to_owned();
        scrub(session, &mut text);
        assert_eq!(text, "token=[cookie sid]; n=1234567");
        take(session);
    }

    /// 209 D14: a site is credentialed when the profile is known to hold it, or when the session
    /// holds that site's profile cookies (here one a save put in its baseline, unknown elsewhere).
    #[test]
    fn credentialed_follows_known_sites_and_the_sessions_baseline() {
        let (on, fresh) = (SessionId::new(), SessionId::new());
        know(["credentialed-test.example".to_owned()]);
        load(
            on,
            0,
            vec![row(
                "loaded-only.example",
                1,
                vec![cookie("sid", "1", "loaded-only.example", false)],
            )],
        );
        if let Some(l) = SESSIONS.lock().get_mut(&on) {
            l.baseline
                .push(cookie("s", "1", "www.baseline-only.example", false));
        }
        assert!(credentialed_url(
            on,
            "https://app.credentialed-test.example/x"
        ));
        assert!(credentialed_url(on, "https://loaded-only.example/"));
        assert!(credentialed_url(on, "https://baseline-only.example/"));
        assert!(!credentialed_url(fresh, "https://baseline-only.example/"));
        assert!(credentialed_url(
            fresh,
            "https://app.credentialed-test.example/x"
        ));
        assert!(!credentialed_url(on, "https://other.example/"));
        assert!(!credentialed_current(on));
        assert!(credentialed_eval(on), "eval with profile cookies loaded");
        assert_eq!(set_current(on, "https://plain.example/"), None);
        assert_eq!(
            set_current(on, "https://www.loaded-only.example/"),
            Some("loaded-only.example".to_owned()),
            "landing on a signed-in site is reported"
        );
        assert!(credentialed_current(on));
        take(on);
        take(fresh);
    }

    #[test]
    fn a_failed_load_backs_off() {
        let session = SessionId::new();
        assert!(retry_due(session));
        failed(session);
        assert!(!retry_due(session));
        loaded(session, LoadedProfile::default());
        assert!(retry_due(session), "a load clears the backoff");
        take(session);
    }
}
