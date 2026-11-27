//! Provider clients (design doc 224 §2, §6). Each one only talks to its provider: no mesh, no
//! vault, no records — the actor (`crate::actor`) owns those and hands a client its credential.
//! HTTP goes through [`Transport`] so tests replay recorded exchanges; nothing here logs a token
//! or puts one anywhere but an `Authorization` header.
//!
//! Every resource carries `theuth-managed` and the node name (DO tags `theuth-managed` +
//! `theuth-node:<name>`, Fly metadata, Kubernetes labels); `list` finds nodes by them and nothing
//! else, so orphans (no local record) show up too.

use async_trait::async_trait;

use crate::types::{K8sOptions, ProviderKind, Quote, Region, SizeOption, Stage};

pub mod catalog;
pub mod digitalocean;
pub mod discovery;
pub mod fly;
pub mod http;
#[cfg(feature = "kubernetes")]
pub mod kubernetes;
#[cfg(feature = "ssh")]
pub mod ssh;
pub mod userdata;

pub use http::{HttpRequest, HttpResponse, ReqwestTransport, Transport};

/// Tag / metadata key on every resource.
pub const TAG_MANAGED: &str = "theuth-managed";

/// How often and how long a client polls a provider (a server booting, an action finishing, a
/// volume detaching). Tests use [`Pace::NOW`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pace {
    /// Pause between polls.
    pub every: std::time::Duration,
    /// Polls before giving up.
    pub tries: u32,
}

impl Pace {
    /// No pause, 5 tries (tests).
    pub const NOW: Self = Self {
        every: std::time::Duration::ZERO,
        tries: 5,
    };

    pub(crate) async fn sleep(&self) {
        if !self.every.is_zero() {
            tokio::time::sleep(self.every).await;
        }
    }
}

impl Default for Pace {
    /// Every 5 s for up to 10 minutes.
    fn default() -> Self {
        Self {
            every: std::time::Duration::from_secs(5),
            tries: 120,
        }
    }
}

/// `theuth-node:<name>`.
pub fn tag_node(name: &str) -> String {
    format!("theuth-node:{name}")
}

/// A provider or input error; its message is what the UI shows (never contains a credential).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudError(pub String);

impl std::fmt::Display for CloudError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CloudError {}

impl From<anyhow::Error> for CloudError {
    fn from(e: anyhow::Error) -> Self {
        Self(format!("{e:#}"))
    }
}

/// A provider result; the error is user-facing.
pub type Result<T, E = CloudError> = std::result::Result<T, E>;

/// 1-40 of `[a-z0-9-]`, starting with a letter, not ending in `-`.
pub fn check_name(name: &str) -> Result<()> {
    let ok = (1..=40).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(CloudError(format!(
            "name {name:?}: 1-40 of a-z, 0-9 and -, starting with a letter, not ending in -"
        )))
    }
}

/// What `create` needs. `ticket` is the key-less one-time pairing ticket (never logged).
#[derive(Clone)]
pub struct CreateReq {
    /// Node name ([`check_name`]).
    pub name: String,
    /// Provider region slug (ignored by Kubernetes).
    pub region: String,
    /// 2, 4 or 8.
    pub memory_gb: u8,
    /// The `/data` disk, GB.
    pub volume_gb: u32,
    /// The pairing ticket (`1.` + base32), or empty.
    pub ticket: String,
    /// Release manifest the node installs from (`latest.json`).
    pub manifest: String,
    /// Engine image (container providers).
    pub image: String,
    /// Kubernetes placement.
    pub k8s: Option<K8sOptions>,
    /// The watchable desktop (VM nodes; container images have none yet and ignore it).
    pub desktop: bool,
}

impl std::fmt::Debug for CreateReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateReq")
            .field("name", &self.name)
            .field("region", &self.region)
            .field("memory_gb", &self.memory_gb)
            .field("volume_gb", &self.volume_gb)
            .field("desktop", &self.desktop)
            .field("ticket", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// What `create` made (persisted in the node record).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Created {
    /// Droplet id / Fly machine id / Deployment name.
    pub server_id: Option<String>,
    /// DO volume id / Fly volume id.
    pub volume_id: Option<String>,
    /// DO firewall id / Fly app name / Kubernetes namespace.
    pub extra: Option<String>,
    /// Public IPv4 (DO).
    pub ip: Option<String>,
    /// The provider's size slug.
    pub size_slug: String,
}

/// What the provider says about one node right now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderStatus {
    /// `None`: no server (destroyed, or only a kept volume).
    pub server_state: Option<String>,
    /// Public IPv4.
    pub ip: Option<String>,
    /// Region slug (Kubernetes: namespace).
    pub region: Option<String>,
    /// The provider's size slug.
    pub size_slug: Option<String>,
    /// Memory, GB.
    pub memory_gb: Option<u8>,
    /// `/data` disk, GB.
    pub volume_gb: Option<u32>,
    /// DO: the firewall's inbound rules (must be empty).
    pub inbound_rules: usize,
}

/// One node a provider's `list` found by its tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    /// Node name.
    pub name: String,
    /// The provider's word for the server; `None` when only the volume is left.
    pub server_state: Option<String>,
    /// Region slug (Kubernetes: namespace).
    pub region: Option<String>,
    /// Public IPv4.
    pub ip: Option<String>,
    /// Memory, GB.
    pub memory_gb: Option<u8>,
    /// `/data` disk, GB.
    pub volume_gb: Option<u32>,
    /// Only its volume is left (destroyed with the disk kept).
    pub volume_only: bool,
}

/// Progress callback: `(stage, line)`.
pub type Say<'a> = &'a (dyn Fn(Stage, &str) + Send + Sync);

/// One provider account (credential already resolved).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Which provider this is.
    fn kind(&self) -> ProviderKind;

    /// Make every resource, the firewall/no-inbound first, then storage, then the server, and
    /// wait until the server runs. Reports [`Stage::Server`] and [`Stage::Storage`] lines via
    /// `say`. Idempotent on retries: existing tagged resources are reused, a running server of
    /// the same name is an error.
    async fn create(&self, req: &CreateReq, say: Say<'_>) -> Result<Created>;

    /// `None` when nothing of the node exists at the provider.
    async fn status(&self, name: &str) -> Result<Option<ProviderStatus>>;

    /// Every node with `theuth-managed` (including volume-only leftovers).
    async fn list(&self) -> Result<Vec<Listed>>;

    /// Reboot the server (the engine restarts; a VM's `ExecStartPre` installs the newest
    /// release, so this is also "update now" when the node does not answer over the mesh).
    async fn restart(&self, name: &str) -> Result<()>;

    /// Change the size to `memory_gb` (2/4/8); may power-cycle. Returns the new slug.
    async fn resize(&self, name: &str, memory_gb: u8) -> Result<String>;

    /// Delete the server and the firewall/app; the volume too unless `keep_volume`. Returns
    /// one line per thing done.
    async fn destroy(&self, name: &str, keep_volume: bool) -> Result<Vec<String>>;
}

/// Static facts per provider (sizes, regions, prices); see [`catalog`].
pub fn sizes(kind: ProviderKind) -> Vec<SizeOption> {
    catalog::sizes(kind)
}

/// Regions per provider; see [`catalog`].
pub fn regions(kind: ProviderKind) -> Vec<Region> {
    catalog::regions(kind)
}

/// The monthly cost from the live `sizes`; see [`catalog::quote`].
pub fn quote(
    kind: ProviderKind,
    sizes: &[SizeOption],
    memory_gb: u8,
    volume_gb: u32,
) -> Result<Quote> {
    catalog::quote(kind, sizes, memory_gb, volume_gb)
}
