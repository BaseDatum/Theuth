//! One browser profile per owner mesh (design doc 209): the user's web view tile and every agent
//! browser session share a cookie jar and per-origin localStorage, kept sealed in the replicated
//! table `web_sites` of `<data_dir>/webprofile.sqlite` (209 §4.1, D2).
//!
//! Public surface consumed by other plugins:
//! - the [`WEBPROFILE_ACTOR`] actor ([`WebProfileActor`]) and its messages [`Push`], [`Load`],
//!   [`Pull`], [`Forget`], [`Clear`], [`Sites`], [`Status`], [`CurrentEpoch`] — or the [`Handle`]
//!   wrapper (`browser`'s `Ctx` loads and saves through it, 209 D4);
//! - the wire types [`Cookie`], [`CookieKey`], [`CookieChange`], [`OriginStorage`], [`SiteJar`],
//!   [`PushResult`], [`SiteSummary`], [`PullResult`], [`PulledCookie`], [`PulledStorage`],
//!   [`ForgottenSite`], [`ForgetResult`], [`ClearResult`], [`ProfileStatus`] (209 §4.1, §4.4);
//! - the event [`WebProfileChanged`] (site names and the epoch, never values);
//! - [`merge::diff`] (the per-cookie delta of 209 §4.2), [`site_of`] (eTLD+1), [`mode`] (config
//!   `browser.profile`).
//!
//! Values never reach `Debug`, `tracing` or events: [`Cookie`] and [`OriginStorage`] print
//! `[redacted]` for every value (209 §9, G3). [`PullResult`] (`web_profile_pull`) is the one
//! answer that carries values, to the owner's app only.

use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub mod merge;
pub mod rpc;
pub mod store;

pub use rpc::rpc_module;
pub use store::Profile;

/// Registry name of the [`WebProfileActor`] (209 §10 P1).
pub const WEBPROFILE_ACTOR: &str = "webprofile";
/// Error message of a push made under an older profile epoch (209 §4.4); the error's data is
/// `{ "epoch": <current> }`.
pub const STALE_EPOCH: &str = "stale-epoch";
/// Error message of a push made under an epoch newer than the current one (a client that made one
/// up); the error's data is `{ "epoch": <current> }`.
pub const UNKNOWN_EPOCH: &str = "unknown-epoch";
/// Error message when the profile is at its site or byte cap and a new site is refused (209 §4.1).
pub const PROFILE_FULL: &str = "profile full";
/// Writer label prefix of a push from a client's tile: `tile:<device name>` (209 §4.1 `writer`).
pub const WRITER_TILE: &str = "tile:";
/// Writer label prefix of a save from an agent browser: `obscura:<engine name>`.
pub const WRITER_OBSCURA: &str = "obscura:";
/// Writer label of the rows `web_profile_forget` and `web_profile_clear` write (the Settings
/// screen; the RPCs name no device).
pub const WRITER_SETTINGS: &str = "settings";

/// Caps of 209 §4.1.
pub mod caps {
    /// Plaintext of one site's live rows together at most.
    pub const ROW_BYTES: usize = 128 * 1024;
    /// Cookies of one site at most.
    pub const SITE_COOKIES: usize = 300;
    /// One origin's localStorage snapshot at most; over it the snapshot is dropped whole.
    pub const ORIGIN_STORAGE_BYTES: usize = 96 * 1024;
    /// Live sites per profile at most.
    pub const SITES: usize = 500;
    /// Plaintext of every live row together at most.
    pub const PROFILE_BYTES: usize = 8 * 1024 * 1024;
    /// 209 §11 Q1 (decided): session cookies (no `expires`) are kept until their site row goes this
    /// long without a write.
    pub const SESSION_COOKIE_IDLE_MS: u64 = 30 * 24 * 3600 * 1000;
    /// Cookie changes in one push at most (checked before any work).
    pub const PUSH_CHANGES: usize = 2000;
    /// localStorage snapshots in one push at most.
    pub const PUSH_STORAGE: usize = 64;
    /// Characters of a push's device name kept.
    pub const DEVICE_CHARS: usize = 64;
    /// One cookie value at most (browsers cap a cookie at 4096 bytes).
    pub const COOKIE_VALUE: usize = 4096;
    /// A removed row (`sealed = NULL`) is deleted this long after its write, at open.
    pub const TOMBSTONE_MS: u64 = 30 * 24 * 3600 * 1000;
    /// A forget marker applies this long after the forget: sets and snapshots of the site from
    /// a writer whose last full read (`basis_ms`) is before it are refused, and
    /// `web_profile_pull` lists it in `forgotten` (209 §4.1 forget marker).
    pub const FORGOTTEN_MS: u64 = TOMBSTONE_MS;
    /// Sites one `web_profile_pull` may name at most.
    pub const PULL_SITES: usize = SITES;
    /// Cookie and snapshot JSON one `web_profile_pull` answer carries at most: further sites go
    /// to the next page (`next`); a page always carries at least one site whole.
    pub const PULL_BYTES: usize = 1024 * 1024;
}

/// One cookie with every attribute (209 D1, §4.1). `value` is a credential: `Debug` redacts it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value (never logged).
    pub value: String,
    /// Domain as the store gives it (a leading `.` is tolerated and ignored for grouping).
    pub domain: String,
    /// `true`: sent to `domain` only, not its subdomains (`__Host-` cookies need it).
    #[serde(default)]
    pub host_only: bool,
    /// Path.
    pub path: String,
    /// Unix seconds, or `null` for a session cookie.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires: Option<i64>,
    /// `Secure`.
    #[serde(default)]
    pub secure: bool,
    /// `HttpOnly`.
    #[serde(default)]
    pub http_only: bool,
    /// `"Strict" | "Lax" | "None" | ""`.
    #[serde(default)]
    pub same_site: String,
    /// CHIPS `Partitioned`; kept although WebKit cannot carry the partition key (209 D1).
    #[serde(default)]
    pub partitioned: bool,
}

impl Cookie {
    /// Its identity `(name, domain, path)` (obscura's too, 209 §4.2).
    pub fn key(&self) -> CookieKey {
        CookieKey {
            name: self.name.clone(),
            domain: self.domain.clone(),
            path: self.path.clone(),
        }
    }
}

impl std::fmt::Debug for Cookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cookie")
            .field("name", &self.name)
            .field("value", &"[redacted]")
            .field("domain", &self.domain)
            .field("host_only", &self.host_only)
            .field("path", &self.path)
            .field("expires", &self.expires)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .field("same_site", &self.same_site)
            .field("partitioned", &self.partitioned)
            .finish()
    }
}

/// A cookie's identity (209 §4.2, §4.5 `CookieKey`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CookieKey {
    /// Cookie name.
    pub name: String,
    /// Domain.
    pub domain: String,
    /// Path.
    pub path: String,
}

/// One change of a push (209 §4.4): `{op:"set", cookie}` | `{op:"remove", name, domain, path}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum CookieChange {
    /// Set (replace) the cookie of this key; an `expires` in the past removes it.
    Set {
        /// The cookie.
        cookie: Cookie,
    },
    /// Remove the cookie of this key.
    Remove {
        /// Cookie name.
        name: String,
        /// Domain.
        domain: String,
        /// Path.
        path: String,
    },
}

impl CookieChange {
    /// The domain the change is about (grouped by [`site_of`]).
    pub fn domain(&self) -> &str {
        match self {
            Self::Set { cookie } => &cookie.domain,
            Self::Remove { domain, .. } => domain,
        }
    }
}

/// One origin's localStorage snapshot (209 §4.1 `origins[]`, §4.4 `storage[]`). Values redacted in `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct OriginStorage {
    /// `scheme://host[:port]`.
    pub origin: String,
    /// `[key, value]` pairs in the page's order.
    pub local_storage: Vec<(String, String)>,
    /// Unix ms of the capture; a newer capture replaces an older one, an older one is ignored.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub captured_ms: u64,
}

impl OriginStorage {
    /// Bytes of keys and values (the 96 KiB cap of 209 §4.1).
    pub fn bytes(&self) -> usize {
        self.local_storage.iter().map(|(k, v)| k.len() + v.len()).sum()
    }
}

impl std::fmt::Debug for OriginStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginStorage")
            .field("origin", &self.origin)
            .field("keys", &self.local_storage.len())
            .field("captured_ms", &self.captured_ms)
            .finish()
    }
}

/// One site as a load answers it (209 §4.1): every live cookie of a registrable domain and its
/// origins' localStorage snapshots, gathered from the site's rows (one row per cookie, one per
/// origin's snapshot).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SiteJar {
    /// Format version, `1`.
    pub v: u32,
    /// Lower-case eTLD+1.
    pub site: String,
    /// Cookies, unique by [`CookieKey`].
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    /// localStorage per origin, unique by `origin`.
    #[serde(default)]
    pub origins: Vec<OriginStorage>,
}

impl SiteJar {
    /// An empty jar for `site`.
    pub fn new(site: impl Into<String>) -> Self {
        Self {
            v: 1,
            site: site.into(),
            cookies: Vec::new(),
            origins: Vec::new(),
        }
    }
}

/// `web_profile_push`'s result (209 §4.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PushResult {
    /// Changes and snapshots applied.
    pub applied: u32,
    /// Sites whose rows were written, sorted.
    pub sites: Vec<String>,
    /// The profile epoch the rows were written under.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch: u64,
    /// `"shared"`, or `"off"` when `browser.profile: off` made the push a no-op (209 D6): the
    /// client stops pushing until it reads `"shared"` again.
    #[serde(default)]
    pub profile: ProfileMode,
    /// Sites whose sets and snapshots were refused because the site was forgotten after the
    /// push's `basis_ms` (or the push named none), sorted; its removes still applied. The client
    /// pulls to learn the forget and does not push those sets again.
    #[serde(default)]
    pub refused: Vec<String>,
}

/// One entry of `web_profile_sites` (209 §4.4): counts, never values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SiteSummary {
    /// Lower-case eTLD+1.
    pub site: String,
    /// Cookies kept.
    pub cookies: u32,
    /// Origins with a localStorage snapshot.
    pub origins: u32,
    /// Unix ms of the row's last write.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_ms: u64,
    /// `tile:<device>` | `obscura:<engine>`.
    pub writer: String,
}

/// One cookie of `web_profile_pull` (209 §4.4, D5): the [`Cookie`] fields, plus its site and
/// the writer and time of its row, so a client skips echoes of its own pushes (`tile:<device>`).
/// `Debug` redacts the value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PulledCookie {
    /// The cookie.
    #[serde(flatten)]
    pub cookie: Cookie,
    /// Lower-case eTLD+1 it belongs to.
    pub site: String,
    /// `tile:<device>` | `obscura:<engine>` of its row.
    pub writer: String,
    /// Unix ms of its row's write.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_ms: u64,
}

/// One origin's localStorage snapshot of `web_profile_pull`, with its site and row writer and time.
/// `Debug` redacts the values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PulledStorage {
    /// The snapshot.
    #[serde(flatten)]
    pub storage: OriginStorage,
    /// Lower-case eTLD+1 of the origin.
    pub site: String,
    /// `tile:<device>` | `obscura:<engine>` of its row.
    pub writer: String,
    /// Unix ms of its row's write.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_ms: u64,
}

/// `web_profile_pull`'s result (209 §4.4, D5): one page of the profile's cookies and snapshots of
/// `sites`. Every site in `sites` is answered whole: a client sets what it gets and deletes from
/// its store the cookies of those sites it did not get (a forgotten site answers nothing).
/// `Debug` redacts every value; nothing logs it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PullResult {
    /// The current profile epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch: u64,
    /// The sites this page covers, sorted: the asked ones as registrable domains, or every site
    /// the profile holds (also one whose cookies all expired) when none were asked. A cookie of
    /// the client's store belongs to a site when its domain (lower case, no leading `.`) is the
    /// site or ends with `.` and the site.
    pub sites: Vec<String>,
    /// Every live cookie of those sites, sorted by site, then name, domain and path.
    pub cookies: Vec<PulledCookie>,
    /// Every localStorage snapshot of those sites, sorted by site, then origin.
    pub storage: Vec<PulledStorage>,
    /// `"shared"`, or `"off"` when `browser.profile: off`: then no site is answered.
    #[serde(default)]
    pub profile: ProfileMode,
    /// Every site forgotten in the last 30 days of this epoch, with when (on every page). The
    /// client deletes from its store the cookies of each one forgotten after its previous
    /// `basis_ms` before it applies this answer's cookies.
    #[serde(default)]
    pub forgotten: Vec<ForgottenSite>,
    /// The engine's clock (Unix ms) just before it read this page: the first page's is the
    /// client's next `basis_ms` once every page arrived.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub now_ms: u64,
    /// More sites follow: pull again with the same `sites` and this as `cursor`. `null` when this
    /// page is the last.
    #[serde(default)]
    pub next: Option<String>,
}

/// One forgotten site of `web_profile_pull` (209 §4.1 forget marker).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ForgottenSite {
    /// Lower-case eTLD+1.
    pub site: String,
    /// Unix ms of the forget (the forgetting engine's clock).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub forgotten_ms: u64,
}

/// `web_profile_forget`'s result (209 §4.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ForgetResult {
    /// The registrable domain forgotten (the asked host's eTLD+1).
    pub site: String,
    /// The current profile epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch: u64,
}

/// `web_profile_clear`'s result (209 §4.4): the new epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ClearResult {
    /// The profile epoch after the clear; pushes under an older one are `stale-epoch`.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch: u64,
}

/// `web_profile_status`'s result (209 §4.4): the mode and the profile's size, cheap (no row is
/// opened), for Settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProfileStatus {
    /// `"shared"` or `"off"` (config `browser.profile`).
    pub profile: ProfileMode,
    /// The current profile epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch: u64,
    /// Sites with live rows (kept also while off, until cleared).
    pub sites: u32,
    /// The engine's clock, Unix ms.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub now_ms: u64,
}

/// One live site as [`Load`] answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedSite {
    /// The jar (expired cookies dropped; session cookies dropped when the row is idle past
    /// [`caps::SESSION_COOKIE_IDLE_MS`]).
    pub jar: SiteJar,
    /// Unix ms of the row's last write: `Ctx` compares it to decide whether to re-apply (209 §4.2).
    pub updated_ms: u64,
    /// Who wrote it last.
    pub writer: String,
}

/// [`Load`]'s answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedProfile {
    /// The current profile epoch; saves are made under it.
    pub epoch: u64,
    /// Live sites, sorted by site.
    pub sites: Vec<LoadedSite>,
}

/// Event: rows of the profile changed (209 §4.4), topic `browser.web-profile-changed`. Site names
/// and the epoch only, never values; not session-scoped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct WebProfileChanged {
    /// Sites whose row changed, sorted.
    pub sites: Vec<String>,
    /// The profile epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch: u64,
}

impl wire::WireEvent for WebProfileChanged {
    const TOPIC: &'static str = "browser.web-profile-changed";
}

/// Why a profile operation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// The write was made under `epoch < current` (sign out everywhere happened elsewhere).
    StaleEpoch {
        /// The current epoch.
        current: u64,
    },
    /// The write was made under `epoch > current`: no engine ever wrote that epoch.
    UnknownEpoch {
        /// The current epoch.
        current: u64,
    },
    /// No site could be written: the profile is at a cap ([`PROFILE_FULL`]).
    Full,
    /// The push breaks a cap of one push or holds a malformed cookie; nothing was written.
    Invalid(String),
    /// A pull or forget the profile refuses before any work; the text is the whole message
    /// (`invalid pull: …`, `invalid forget: …`).
    Rejected(String),
    /// No store or no vault yet (starting), or `browser.profile: off` for a load.
    Unavailable(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleEpoch { .. } => f.write_str(STALE_EPOCH),
            Self::UnknownEpoch { .. } => f.write_str(UNKNOWN_EPOCH),
            Self::Full => f.write_str(PROFILE_FULL),
            Self::Invalid(why) => write!(f, "invalid push: {why}"),
            Self::Rejected(why) => f.write_str(why),
            Self::Unavailable(why) => write!(f, "browser profile unavailable: {why}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// Config `browser.profile` (209 D6): `shared` (default) or `off` (no push, load or save; rows stay).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ProfileMode {
    /// One profile for the tile and every agent browser of the mesh.
    #[default]
    Shared,
    /// Nothing is pushed, loaded or saved.
    Off,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct BrowserSection {
    profile: ProfileMode,
}

/// The current `browser.profile` (209 D6); `shared` when the section or key is absent.
pub fn mode(rt: &Runtime) -> ProfileMode {
    rt.section::<BrowserSection>("browser")
        .map(|s| s.profile)
        .unwrap_or_default()
}

/// The registrable domain (eTLD+1, lower case) of `host` by the public suffix list (its private
/// section included, so `user.github.io` is its own site). An IP address (with or without `[]`),
/// a single-label host (`localhost`) and a host that is itself a public suffix are their own
/// site. A leading or trailing `.` is ignored.
pub fn site_of(host: &str) -> String {
    let host = host.trim().trim_start_matches('.').trim_end_matches('.');
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
        .to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') {
        return host;
    }
    psl::domain_str(&host).unwrap_or(&host).to_owned()
}

/// Whether cookies may be scoped to `host` (lower case, no leading `.`): an IP address, a
/// single-label host, or a host with a registrable domain — never a bare public suffix
/// (`co.uk`, `github.io`), whose cookie would reach every site under it.
pub fn cookie_host(host: &str) -> bool {
    !host.is_empty()
        && (host.parse::<std::net::IpAddr>().is_ok()
            || !host.contains('.')
            || psl::domain_str(host).is_some())
}

/// Push changes made under `epoch` by `writer` (209 §4.2). Grouped by site and merged into each row.
#[derive(Clone, Debug)]
pub struct Push {
    /// `tile:<device>` | `obscura:<engine>`.
    pub writer: String,
    /// The epoch the writer last saw ([`LoadedProfile::epoch`], or the client's).
    pub epoch: u64,
    /// Cookie changes.
    pub changes: Vec<CookieChange>,
    /// localStorage snapshots.
    pub storage: Vec<OriginStorage>,
    /// Unix ms when the writer last read the whole profile (an agent session's load; `None` for
    /// a tile). A site forgotten after it is not written again by this push, and a row it wrote
    /// that reaches an engine after the forget is dead there (209 §4.1 forget marker).
    pub basis_ms: Option<u64>,
}

/// Read live rows: every one (`sites: None`) or those named.
#[derive(Clone, Debug, Default)]
pub struct Load {
    /// Sites (eTLD+1) to read; `None` = all.
    pub sites: Option<Vec<String>>,
}

/// The `web_profile_sites` list: counts, never values.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sites;

/// The current profile epoch.
#[derive(Clone, Copy, Debug, Default)]
pub struct CurrentEpoch;

/// `web_profile_status`: mode, epoch and live site count, without opening a row.
#[derive(Clone, Copy, Debug, Default)]
pub struct Status;

/// One page of `web_profile_pull` (209 §4.4, D5): the live cookies and snapshots of `sites` (any
/// hosts, grouped by registrable domain), or of every site, from the site `cursor` on.
#[derive(Clone, Debug, Default)]
pub struct Pull {
    /// Sites to answer; `None` = all.
    pub sites: Option<Vec<String>>,
    /// The previous page's `next`; `None` = the first page.
    pub cursor: Option<String>,
}

/// `web_profile_forget` (209 §4.4, §6): tombstone every row of the site of `site` and mark it
/// forgotten, so a stale save or push does not write it back.
#[derive(Clone, Debug)]
pub struct Forget {
    /// A host or registrable domain.
    pub site: String,
}

/// `web_profile_clear` (209 §4.4, §6): sign out everywhere — the epoch goes up by one and every
/// row is tombstoned.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clear;

/// Housekeeping, told by the plugin every minute: when the mesh key changed since the store last
/// looked, every row moves to the ids of the new key ([`Profile::maintain`], 209 §4.1).
#[derive(Clone, Copy, Debug, Default)]
pub struct Maintain;

/// Rows of `web_sites` a peer's batch changed (their ids; `None` = unknown, the feed lagged):
/// published as [`WebProfileChanged`] with the sites they belong to, so every engine learns a
/// sign-in made elsewhere within seconds (209 D14).
#[derive(Clone, Debug, Default)]
pub struct Replicated(pub Option<Vec<String>>);

/// The [`WEBPROFILE_ACTOR`] actor: the store (when open), the bus for [`WebProfileChanged`].
/// A push, a forget, the epoch and the status run on the actor's turn; everything that reads
/// every row (a full load or pull, the Settings list, sign out everywhere's site list, a key
/// rotation's move, a peer's batch) runs on its own task with the store's pool, so a session's
/// load or a push is never queued behind a scan (209 §4.1).
#[derive(kameo::Actor)]
pub struct WebProfileActor {
    rt: Option<Runtime>,
    profile: Result<Arc<Profile>, String>,
    /// When a push refused as `profile full` last started a full read.
    rescanned: Option<std::time::Instant>,
}

impl WebProfileActor {
    /// An actor over `profile` (`Err` = why there is none); `rt` publishes [`WebProfileChanged`].
    pub fn new(rt: Option<Runtime>, profile: Result<Profile, String>) -> Self {
        Self {
            rt,
            profile: profile.map(Arc::new),
            rescanned: None,
        }
    }

    fn off(&self) -> bool {
        self.rt.as_ref().is_some_and(|rt| mode(rt) == ProfileMode::Off)
    }

    fn mode(&self) -> ProfileMode {
        if self.off() {
            ProfileMode::Off
        } else {
            ProfileMode::Shared
        }
    }

    fn profile(&self) -> Result<&Arc<Profile>, ProfileError> {
        if self.off() {
            return Err(ProfileError::Unavailable("browser.profile is off".into()));
        }
        self.store()
    }

    /// The store whatever `browser.profile` says: forget and clear work while it is off (209 D6:
    /// off leaves rows as they are until cleared), and so do reads that are not a push.
    fn store(&self) -> Result<&Arc<Profile>, ProfileError> {
        self.profile
            .as_ref()
            .map_err(|e| ProfileError::Unavailable(e.clone()))
    }

    /// A write saw a new mesh key: the move runs on its own task ([`Profile::maintain`]).
    fn fold_if_owed(&self) {
        if let Ok(p) = &self.profile
            && p.fold_owed()
        {
            let p = p.clone();
            tokio::spawn(async move {
                if let Err(e) = p.maintain().await {
                    tracing::warn!("webprofile: maintenance failed: {e}");
                }
            });
        }
    }
}

async fn publish(rt: Option<&Runtime>, sites: Vec<String>, epoch: u64) {
    if let Some(rt) = rt {
        rt.publish(WebProfileChanged { sites, epoch }).await;
    }
}

/// How often a push refused as `profile full` may start a full read: rows of an older epoch or
/// of a forgotten site count in the stats until a full read tombstones them (209 §4.1 caps).
const RESCAN_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

impl Message<Push> for WebProfileActor {
    type Reply = Result<PushResult, ProfileError>;
    /// `browser.profile: off` answers an empty result with `profile: "off"` (209 D6); otherwise
    /// the push is checked ([`merge::validate`]) before any work, then merged. A push refused as
    /// `profile full` starts a full read on its own task (at most once a minute), so dead rows
    /// still counted leave the stats for the next push.
    async fn handle(&mut self, mut msg: Push, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.off() {
            let epoch = match &self.profile {
                Ok(p) => p.epoch().await.unwrap_or(msg.epoch),
                Err(_) => msg.epoch,
            };
            return Ok(PushResult {
                epoch,
                profile: ProfileMode::Off,
                ..PushResult::default()
            });
        }
        merge::validate(&mut msg)?;
        let p = self.profile()?.clone();
        let out = p.push(msg).await;
        self.fold_if_owed();
        match &out {
            Ok(r) if !r.sites.is_empty() => {
                publish(self.rt.as_ref(), r.sites.clone(), r.epoch).await;
            }
            Err(ProfileError::Full) if self.rescanned.is_none_or(|t| t.elapsed() > RESCAN_EVERY) => {
                self.rescanned = Some(std::time::Instant::now());
                tokio::spawn(async move {
                    if let Err(e) = p.load(None).await {
                        tracing::debug!("webprofile: full read after a full profile failed: {e}");
                    }
                });
            }
            _ => {}
        }
        out
    }
}

impl Message<Load> for WebProfileActor {
    type Reply = DelegatedReply<Result<LoadedProfile, ProfileError>>;
    /// On its own task, whatever `browser.profile` says: a session's load checks the mode itself,
    /// and the browser reads sites back after a forget also while it is off (209 §4.6).
    async fn handle(&mut self, msg: Load, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let p = self.store().cloned();
        ctx.spawn(async move { p?.load(msg.sites.as_deref()).await })
    }
}

impl Message<Sites> for WebProfileActor {
    type Reply = DelegatedReply<Result<Vec<SiteSummary>, ProfileError>>;
    /// `browser.profile: off` lists nothing (209 D6); otherwise a full read on its own task.
    async fn handle(&mut self, _: Sites, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let p = if self.off() {
            None
        } else {
            Some(self.store().cloned())
        };
        ctx.spawn(async move {
            match p {
                None => Ok(Vec::new()),
                Some(p) => p?.sites().await,
            }
        })
    }
}

impl Message<CurrentEpoch> for WebProfileActor {
    type Reply = Result<u64, ProfileError>;
    async fn handle(&mut self, _: CurrentEpoch, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.profile()?.epoch().await
    }
}

impl Message<Status> for WebProfileActor {
    type Reply = Result<ProfileStatus, ProfileError>;
    /// The mode, the epoch and the live sites of the local stats; also while off.
    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (epoch, sites) = self.store()?.status().await?;
        Ok(ProfileStatus {
            profile: self.mode(),
            epoch,
            sites,
            now_ms: store::now_ms(),
        })
    }
}

impl Message<Pull> for WebProfileActor {
    type Reply = DelegatedReply<Result<PullResult, ProfileError>>;
    /// At most [`caps::PULL_SITES`] named sites (`invalid pull: …` otherwise, before any work);
    /// `browser.profile: off` answers no site with `profile: "off"` (209 D6). The page is read on
    /// its own task.
    async fn handle(&mut self, msg: Pull, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (off, p) = (self.off(), self.store().cloned());
        ctx.spawn(async move {
            if let Some(sites) = &msg.sites
                && sites.len() > caps::PULL_SITES
            {
                return Err(ProfileError::Rejected(format!(
                    "invalid pull: {} sites > {}",
                    sites.len(),
                    caps::PULL_SITES
                )));
            }
            if off {
                let epoch = match &p {
                    Ok(p) => p.epoch().await.unwrap_or_default(),
                    Err(_) => 0,
                };
                return Ok(PullResult {
                    epoch,
                    profile: ProfileMode::Off,
                    now_ms: store::now_ms(),
                    ..PullResult::default()
                });
            }
            p?.pull_page(msg.sites.as_deref(), msg.cursor.as_deref())
                .await
        })
    }
}

impl Message<Forget> for WebProfileActor {
    type Reply = Result<ForgetResult, ProfileError>;
    /// Tombstones the site's rows on this engine (replication carries them to the others) and
    /// publishes [`WebProfileChanged`] naming it, also with `browser.profile: off`.
    async fn handle(&mut self, msg: Forget, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let site = site_of(&msg.site);
        if site.is_empty() {
            return Err(ProfileError::Rejected("invalid forget: no site".into()));
        }
        let epoch = self.store()?.forget(&site, WRITER_SETTINGS).await?;
        self.fold_if_owed();
        publish(self.rt.as_ref(), vec![site.clone()], epoch).await;
        Ok(ForgetResult { site, epoch })
    }
}

impl Message<Clear> for WebProfileActor {
    type Reply = DelegatedReply<Result<ClearResult, ProfileError>>;
    /// Sign out everywhere, also with `browser.profile: off`, on its own task (it lists the live
    /// sites first); publishes [`WebProfileChanged`] with the sites that were live and the new
    /// epoch.
    async fn handle(&mut self, _: Clear, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (p, rt) = (self.store().cloned(), self.rt.clone());
        ctx.spawn(async move {
            let p = p?;
            let was: Vec<String> = p.sites().await?.into_iter().map(|s| s.site).collect();
            let epoch = p.clear().await?;
            publish(rt.as_ref(), was, epoch).await;
            Ok(ClearResult { epoch })
        })
    }
}

impl Message<Maintain> for WebProfileActor {
    type Reply = ();
    /// On its own task ([`Profile::maintain`]; moves are one at a time).
    async fn handle(&mut self, _: Maintain, _: &mut Context<Self, Self::Reply>) {
        if let Ok(p) = &self.profile {
            let p = p.clone();
            tokio::spawn(async move {
                if let Err(e) = p.maintain().await {
                    tracing::warn!("webprofile: maintenance failed: {e}");
                }
            });
        }
    }
}

impl Message<Replicated> for WebProfileActor {
    type Reply = ();
    /// On its own task: read the sites the batch names and publish them, then re-key rows a
    /// lagging engine wrote under a retained key.
    async fn handle(&mut self, msg: Replicated, _: &mut Context<Self, Self::Reply>) {
        let Ok(p) = &self.profile else { return };
        let (p, rt) = (p.clone(), self.rt.clone());
        tokio::spawn(async move {
            match p.changed(msg.0.as_deref()).await {
                Ok(Some((sites, epoch))) => publish(rt.as_ref(), sites, epoch).await,
                Ok(None) => {}
                Err(e) => tracing::debug!("webprofile: replicated rows not read: {e}"),
            }
            // A lagging engine's rows under a key this engine already moved off: re-keyed now,
            // while the old key is still held (209 §4.1 rotation).
            if let Err(e) = p.rekey_replicated(msg.0.as_deref()).await {
                tracing::warn!("webprofile: replicated rows not re-keyed: {e}");
            }
        });
    }
}

/// A cloneable wrapper other plugins call instead of naming messages; resolved lazily.
#[derive(Clone)]
pub struct Handle(ActorRef<WebProfileActor>);

impl Handle {
    /// The running actor, or `None` when the plugin is not started.
    pub fn lookup() -> Option<Self> {
        ActorRef::<WebProfileActor>::lookup(WEBPROFILE_ACTOR)
            .ok()
            .flatten()
            .map(Self)
    }

    /// Wrap an actor (tests).
    pub fn from_actor(actor: ActorRef<WebProfileActor>) -> Self {
        Self(actor)
    }

    /// The actor itself.
    pub fn actor(&self) -> &ActorRef<WebProfileActor> {
        &self.0
    }

    /// [`Push`].
    pub async fn push(&self, msg: Push) -> Result<PushResult, ProfileError> {
        self.0.ask(msg).await.map_err(send_err)
    }

    /// [`Load`].
    pub async fn load(&self, sites: Option<Vec<String>>) -> Result<LoadedProfile, ProfileError> {
        self.0.ask(Load { sites }).await.map_err(send_err)
    }

    /// [`Sites`].
    pub async fn sites(&self) -> Result<Vec<SiteSummary>, ProfileError> {
        self.0.ask(Sites).await.map_err(send_err)
    }

    /// The first page of [`Pull`].
    pub async fn pull(&self, sites: Option<Vec<String>>) -> Result<PullResult, ProfileError> {
        self.pull_page(sites, None).await
    }

    /// One page of [`Pull`], from `cursor` (the previous page's `next`).
    pub async fn pull_page(
        &self,
        sites: Option<Vec<String>>,
        cursor: Option<String>,
    ) -> Result<PullResult, ProfileError> {
        self.0
            .ask(Pull { sites, cursor })
            .await
            .map_err(send_err)
    }

    /// [`Status`].
    pub async fn status(&self) -> Result<ProfileStatus, ProfileError> {
        self.0.ask(Status).await.map_err(send_err)
    }

    /// [`Forget`].
    pub async fn forget(&self, site: impl Into<String>) -> Result<ForgetResult, ProfileError> {
        self.0
            .ask(Forget { site: site.into() })
            .await
            .map_err(send_err)
    }

    /// [`Clear`].
    pub async fn clear(&self) -> Result<ClearResult, ProfileError> {
        self.0.ask(Clear).await.map_err(send_err)
    }

    /// [`CurrentEpoch`].
    pub async fn epoch(&self) -> Result<u64, ProfileError> {
        self.0.ask(CurrentEpoch).await.map_err(send_err)
    }
}

fn send_err<M>(e: kameo::error::SendError<M, ProfileError>) -> ProfileError {
    match e {
        kameo::error::SendError::HandlerError(e) => e,
        other => ProfileError::Unavailable(format!("webprofile actor: {other}")),
    }
}

/// Tell `actor` about every peer batch that touches `web_sites` ([`Replicated`]): a sign-in made
/// on another engine reaches this engine's bus as [`WebProfileChanged`] (209 D14). Runs until the
/// feed or the actor ends.
pub async fn watch_replica(handle: replica::ReplicaHandle, actor: ActorRef<WebProfileActor>) {
    use tokio::sync::broadcast::error::RecvError;
    let mut feed = handle.subscribe();
    let me = handle.origin().clone();
    loop {
        let ids = match feed.recv().await {
            Ok(batch) if batch.origin == me => continue,
            Ok(batch) => {
                let ids: std::collections::BTreeSet<String> = batch
                    .changes
                    .iter()
                    .filter(|c| store::TABLES.contains(&c.table.as_str()))
                    .map(|c| c.pk.clone())
                    .collect();
                if ids.is_empty() {
                    continue;
                }
                Some(ids.into_iter().collect())
            }
            Err(RecvError::Lagged(_)) => None,
            Err(RecvError::Closed) => return,
        };
        if actor.tell(Replicated(ids)).await.is_err() {
            return;
        }
    }
}

/// Tell `actor` [`Maintain`] after every change of the vault (a peer's key frame, a rotation, a
/// pairing adopting a keyring): a new mesh key is noticed at once, so the rows move to it and the
/// old key is held ([`secrets::RetireHold`]) until they did — well before the vault's 10-minute
/// retire grace can end (209 §4.1 rotation). Runs until the vault or the actor ends.
pub async fn watch_keys(vault: Arc<secrets::Vault>, actor: ActorRef<WebProfileActor>) {
    let mut changes = vault.watch_changes();
    drop(vault);
    while changes.changed().await.is_ok() {
        if actor.tell(Maintain).await.is_err() {
            return;
        }
    }
}

/// Declare the wire contract ([`WebProfileChanged`]) before producers start.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<WebProfileChanged>().await
}

/// How often the plugin tells the actor [`Maintain`].
const MAINTAIN_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// The plugin: opens `webprofile.sqlite`, replication, and registers [`WEBPROFILE_ACTOR`].
pub struct WebProfilePlugin;

#[async_trait::async_trait]
impl runtime::Plugin for WebProfilePlugin {
    fn name(&self) -> &'static str {
        "webprofile"
    }

    async fn declare_wire(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        // `start` must not block on other plugins: the vault is awaited by a task, the actor
        // answers "starting" until the store is open.
        let actor = WebProfileActor::spawn(WebProfileActor::new(
            Some(rt.clone()),
            Err("the browser profile is starting".into()),
        ));
        actor.register(WEBPROFILE_ACTOR)?;
        let rt = rt.clone();
        tokio::spawn(async move {
            let profile = open(&rt, &actor).await;
            if let Err(e) = &profile {
                tracing::warn!("webprofile: {e}; the browser profile is off");
            }
            let keys = profile.as_ref().ok().map(|p| p.vault().clone());
            let _ = actor.tell(Opened(profile)).await;
            if let Some(vault) = keys {
                tokio::spawn(watch_keys(vault, actor.clone()));
            }
            let mut tick = tokio::time::interval(MAINTAIN_EVERY);
            tick.tick().await;
            loop {
                tokio::select! {
                    () = rt.shutdown.cancelled() => return,
                    _ = tick.tick() => {}
                }
                if actor.tell(Maintain).await.is_err() {
                    return;
                }
            }
        });
        Ok(())
    }
}

/// The store is open (or failed to open): replaces the actor's "starting".
struct Opened(Result<Profile, String>);

impl Message<Opened> for WebProfileActor {
    type Reply = ();
    async fn handle(&mut self, msg: Opened, _: &mut Context<Self, Self::Reply>) {
        self.profile = msg.0.map(Arc::new);
    }
}

/// Open the store once the vault is up, and attach replication once the replica plugin is up (it
/// may start after this one); a peer's rows then reach `actor` through [`watch_replica`].
async fn open(rt: &Runtime, actor: &ActorRef<WebProfileActor>) -> Result<Profile, String> {
    let vault: Arc<secrets::Vault> = secrets::handle(rt)
        .await
        .ok_or_else(|| "secrets vault unavailable".to_owned())?;
    let profile = Profile::open(&rt.data_dir.join(store::DB), vault)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let pool = profile.pool().clone();
    let rt2 = rt.clone();
    let actor = actor.clone();
    tokio::spawn(async move {
        if rt2
            .lookup_when::<replica::Replica>(replica::REPLICA_ACTOR)
            .await
            .is_none()
        {
            return;
        }
        match replica::attach(&pool, store::TABLES).await {
            Ok(handle) => {
                tracing::info!("webprofile: web_sites replicated");
                watch_replica(handle, actor).await;
            }
            Err(e) => tracing::info!("webprofile: replication unavailable ({e}); local only"),
        }
    });
    Ok(profile)
}
