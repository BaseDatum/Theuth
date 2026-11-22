//! One browser profile per owner mesh (design doc 209): the user's web view tile and every agent
//! browser session share a cookie jar and per-origin localStorage, kept sealed in the replicated
//! table `web_sites` of `<data_dir>/webprofile.sqlite` (209 §4.1, D2).
//!
//! Public surface consumed by other plugins:
//! - the [`WEBPROFILE_ACTOR`] actor ([`WebProfileActor`]) and its messages [`Push`], [`Load`],
//!   [`Sites`], [`CurrentEpoch`] — or the [`Handle`] wrapper (`browser`'s `Ctx` loads and saves
//!   through it, 209 D4);
//! - the wire types [`Cookie`], [`CookieKey`], [`CookieChange`], [`OriginStorage`], [`SiteJar`],
//!   [`PushResult`], [`SiteSummary`] (209 §4.1, §4.4);
//! - the event [`WebProfileChanged`] (site names and the epoch, never values);
//! - [`merge::diff`] (the per-cookie delta of 209 §4.2), [`site_of`] (eTLD+1), [`mode`] (config
//!   `browser.profile`).
//!
//! Values never reach `Debug`, `tracing` or events: [`Cookie`] and [`OriginStorage`] print
//! `[redacted]` for every value (209 §9, G3).

use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
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
/// Error message when the profile is at its site or byte cap and a new site is refused (209 §4.1).
pub const PROFILE_FULL: &str = "profile full";
/// Writer label prefix of a push from a client's tile: `tile:<device name>` (209 §4.1 `writer`).
pub const WRITER_TILE: &str = "tile:";
/// Writer label prefix of a save from an agent browser: `obscura:<engine name>`.
pub const WRITER_OBSCURA: &str = "obscura:";

/// Caps of 209 §4.1.
pub mod caps {
    /// A row's plaintext (`SiteJar` JSON) at most.
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

/// The plaintext of one `web_sites` row (209 §4.1): every cookie of a registrable domain and its
/// origins' localStorage snapshots.
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
    /// Sites whose row was written, sorted.
    pub sites: Vec<String>,
    /// The profile epoch the rows were written under.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch: u64,
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
    /// No site could be written: the profile is at a cap ([`PROFILE_FULL`]).
    Full,
    /// `browser.profile: off`, no store or no vault.
    Unavailable(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleEpoch { .. } => f.write_str(STALE_EPOCH),
            Self::Full => f.write_str(PROFILE_FULL),
            Self::Unavailable(why) => write!(f, "browser profile unavailable: {why}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// Config `browser.profile` (209 D6): `shared` (default) or `off` (no push, load or save; rows stay).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

/// The registrable domain (eTLD+1, lower case) of `host` by the public suffix list; a host with no
/// registrable domain (an IP address, `localhost`) is its own site. A leading `.` is ignored.
pub fn site_of(host: &str) -> String {
    let host = host.trim().trim_start_matches('.').trim_end_matches('.');
    let host = host.to_ascii_lowercase();
    psl::domain_str(&host).unwrap_or(&host).to_owned()
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

/// The [`WEBPROFILE_ACTOR`] actor: the store (when open), the bus for [`WebProfileChanged`].
#[derive(kameo::Actor)]
pub struct WebProfileActor {
    rt: Option<Runtime>,
    profile: Result<Profile, String>,
}

impl WebProfileActor {
    /// An actor over `profile` (`Err` = why there is none); `rt` publishes [`WebProfileChanged`].
    pub fn new(rt: Option<Runtime>, profile: Result<Profile, String>) -> Self {
        Self { rt, profile }
    }

    fn profile(&self) -> Result<&Profile, ProfileError> {
        if let Some(rt) = &self.rt
            && mode(rt) == ProfileMode::Off
        {
            return Err(ProfileError::Unavailable("browser.profile is off".into()));
        }
        self.profile
            .as_ref()
            .map_err(|e| ProfileError::Unavailable(e.clone()))
    }
}

impl Message<Push> for WebProfileActor {
    type Reply = Result<PushResult, ProfileError>;
    async fn handle(&mut self, msg: Push, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let out = self.profile()?.push(msg).await?;
        if !out.sites.is_empty()
            && let Some(rt) = &self.rt
        {
            rt.publish(WebProfileChanged {
                sites: out.sites.clone(),
                epoch: out.epoch,
            })
            .await;
        }
        Ok(out)
    }
}

impl Message<Load> for WebProfileActor {
    type Reply = Result<LoadedProfile, ProfileError>;
    async fn handle(&mut self, msg: Load, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.profile()?.load(msg.sites.as_deref()).await
    }
}

impl Message<Sites> for WebProfileActor {
    type Reply = Result<Vec<SiteSummary>, ProfileError>;
    async fn handle(&mut self, _: Sites, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.profile()?.sites().await
    }
}

impl Message<CurrentEpoch> for WebProfileActor {
    type Reply = Result<u64, ProfileError>;
    async fn handle(&mut self, _: CurrentEpoch, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.profile()?.epoch().await
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

/// Declare the wire contract ([`WebProfileChanged`]) before producers start.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<WebProfileChanged>().await
}

/// The plugin: opens `webprofile.sqlite`, the salt, replication, and registers [`WEBPROFILE_ACTOR`].
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
            let profile = open(&rt).await;
            if let Err(e) = &profile {
                tracing::warn!("webprofile: {e}; the browser profile is off");
            }
            let _ = actor.tell(Opened(profile)).await;
        });
        Ok(())
    }
}

/// The store is open (or failed to open): replaces the actor's "starting".
struct Opened(Result<Profile, String>);

impl Message<Opened> for WebProfileActor {
    type Reply = ();
    async fn handle(&mut self, msg: Opened, _: &mut Context<Self, Self::Reply>) {
        self.profile = msg.0;
    }
}

/// Open the store, wait for the vault, read or create the salt, and attach replication once the
/// replica plugin is up (it may start after this one).
async fn open(rt: &Runtime) -> Result<Profile, String> {
    let vault: Arc<secrets::Vault> = secrets::handle(rt)
        .await
        .ok_or_else(|| "secrets vault unavailable".to_owned())?;
    let profile = Profile::open(&rt.data_dir.join(store::DB), vault)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let pool = profile.pool().clone();
    let rt2 = rt.clone();
    tokio::spawn(async move {
        if rt2
            .lookup_when::<replica::Replica>(replica::REPLICA_ACTOR)
            .await
            .is_none()
        {
            return;
        }
        match replica::attach(&pool, store::TABLES).await {
            Ok(_) => tracing::info!("webprofile: web_sites replicated"),
            Err(e) => tracing::info!("webprofile: replication unavailable ({e}); local only"),
        }
    });
    Ok(profile)
}
