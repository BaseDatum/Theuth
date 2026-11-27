//! Wire types of the `cloud_*` RPC family and the `cloudnodes.events` topic (design doc 224 §13).
//! Shared contract between the provider clients (`providers/`), the actor/job runner and the UI
//! (`crates/desktop/ui/src/components/cloud/`). Money is USD per month, list prices.

use serde::{Deserialize, Serialize};

/// A provider this build knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Droplet + Block Storage volume + Cloud Firewall; native install under systemd.
    Digitalocean,
    /// App + Volume + Machine (Machines API); the engine image. Beta: never live-tested.
    Fly,
    /// A one-replica Deployment via kubectl (`sandbox::k8s`). Desktop only.
    Kubernetes,
    /// Lightsail instance + disk. Coming soon: listed, not creatable.
    Lightsail,
}

/// The 2 GB minimum and the two steps up (owner decision: 2 GB is the default everywhere).
pub const SIZES_GB: [u8; 3] = [2, 4, 8];
/// The `/data` volume every node gets.
pub const DEFAULT_VOLUME_GB: u32 = 10;

/// One size a provider offers for a node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SizeOption {
    /// 2, 4 or 8.
    pub memory_gb: u8,
    /// Virtual CPUs.
    pub vcpus: u8,
    /// The provider's slug (`s-1vcpu-2gb`, `shared-cpu-1x`, …); for Kubernetes `2Gi`.
    pub slug: String,
    /// Server only; `None` for Kubernetes (the cluster's cost).
    pub monthly_usd: Option<f64>,
}

/// `cloud_sizes`: the sizes of one provider (and region), priced live from the provider's API
/// on every call (no cache, no price list in the app).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SizeList {
    /// 2, 4 and 8 GB; `monthly_usd` set only when `priced`.
    pub sizes: Vec<SizeOption>,
    /// The prices came from the provider just now.
    pub priced: bool,
    /// Why they did not (no credential, the API failed); `None` when priced or not priceable.
    pub error: Option<String>,
}

/// One region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Region {
    /// The provider's slug (`lon1`, `lhr`).
    pub id: String,
    /// "London".
    pub label: String,
    /// ISO country code, for the flag (`GB`).
    pub country: String,
}

/// A credential found on this machine (read-only discovery; nothing is copied until the user
/// presses "Use it" → `cloud_import_credential`). Never carries the secret itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Discovered {
    /// Stable id to pass back to `cloud_import_credential` (`doctl:default`, `flyctl`, `kube:<context>`).
    pub id: String,
    /// What the UI says: "doctl login", "flyctl login", "kubeconfig context rwx-dev".
    pub label: String,
    /// More: `context "default"`, `org personal`, `3 contexts`.
    pub detail: Option<String>,
    /// The file it was read from (`~/.config/doctl/config.yaml`).
    pub path: String,
}

/// Where the credential the engine would use comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    /// `Sign in with DigitalOcean` (tokens from the relay's code exchange; refreshable).
    Oauth,
    /// Pasted by the user.
    Pasted,
    /// Copied from a discovered CLI login on the user's press.
    Imported,
    /// Kubernetes: kubectl and the user's kubeconfig (nothing stored).
    Kubeconfig,
}

/// One provider as the wizard shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderInfo {
    /// Which provider.
    pub provider: ProviderKind,
    /// "DigitalOcean".
    pub label: String,
    /// Can create nodes in this build (Lightsail: false).
    pub available: bool,
    /// Never live-tested (Fly.io).
    pub beta: bool,
    /// Only offered where kubectl runs (desktop engines).
    pub desktop_only: bool,
    /// A usable credential is in the vault (or, for Kubernetes, kubectl has contexts).
    pub connected: bool,
    /// Where the stored credential came from.
    pub source: Option<CredentialSource>,
    /// Credentials found on this machine, offered first on desktop.
    pub discovered: Vec<Discovered>,
    /// "Sign in with DigitalOcean" works (the relay is reachable and this provider has OAuth).
    pub oauth: bool,
    /// 2/4/8 GB, unpriced (`cloud_sizes` prices them live).
    pub sizes: Vec<SizeOption>,
    /// Regions offered.
    pub regions: Vec<Region>,
    /// Disk price per GB-month (`None` for Kubernetes).
    pub volume_usd_per_gb: Option<f64>,
}

/// Kubernetes-only options (wizard step 2, Kubernetes variant).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct K8sOptions {
    /// kubeconfig context (default: the current one).
    pub context: Option<String>,
    /// Namespace (default: the context's).
    pub namespace: Option<String>,
    /// Create `namespace` when it does not exist.
    #[serde(default)]
    pub create_namespace: bool,
    /// StorageClass of the volumes (default: the cluster's default).
    pub storage_class: Option<String>,
    /// Add a NetworkPolicy: no ingress; egress to DNS and the internet, not the cluster.
    #[serde(default)]
    pub deny_cluster_network: bool,
}

/// What the user asked for (wizard step 2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NodeSpec {
    /// Where it runs.
    pub provider: ProviderKind,
    /// 1-40 of `[a-z0-9-]`, starting with a letter, not ending in `-` (also the mesh name).
    pub name: String,
    /// Provider region slug; ignored for Kubernetes.
    pub region: String,
    /// 2, 4 or 8.
    pub memory_gb: u8,
    /// Default [`DEFAULT_VOLUME_GB`].
    pub volume_gb: Option<u32>,
    /// Kubernetes only.
    pub k8s: Option<K8sOptions>,
    /// A watchable desktop on the node (Xvfb + noVNC behind loopback; watched over the mesh).
    /// `None` = on (the default).
    #[serde(default)]
    pub desktop: Option<bool>,
}

/// The monthly cost the create button names (owner: the press on "Create node · $13/mo" is the
/// approval).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Quote {
    /// Which provider.
    pub provider: ProviderKind,
    /// The size quoted.
    pub size_slug: String,
    /// Server per month, live from the provider; `None` when unknown.
    pub server_usd: Option<f64>,
    /// Disk per month; `None` for Kubernetes.
    pub disk_usd: Option<f64>,
    /// `server + disk`, rounded to cents; `None` for Kubernetes.
    pub total_usd: Option<f64>,
}

/// The create job's stages, in order (wizard step ④).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Server (Droplet / Machine / Deployment) created and running.
    Server,
    /// The `/data` volume attached.
    Storage,
    /// Installing the newest signed theuth release (ends when the node dials the mesh).
    Install,
    /// The node presented its ticket; waiting until it is reachable.
    Join,
    /// The node's sealed vault gets the master password and this engine's device unlock
    /// (design doc 225); skipped where the node's release has no sealed vault yet.
    Vault,
    /// A reachable mesh peer.
    Ready,
}

/// How the node's sealed vault is protected (design doc 225), as the create job and the list
/// last saw it. Never the password itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaultProtection {
    /// The create job holds a master password (in memory) to add once the node joined.
    Pending,
    /// The master password and unlock from the creating engine, both confirmed on the node.
    Protected,
    /// Unlock from the creating engine only: the user chose no master password.
    DeviceOnly,
    /// A master password was asked for and is not set (the job was interrupted, this engine
    /// restarted, the call failed), or no unlock method could be confirmed: set one.
    NeedsPassword,
    /// The master password is set; unlock from the creating engine is not confirmed.
    PasswordOnly,
    /// The node's theuth release has no sealed vault yet (no `vault_status`).
    Unsupported,
}

/// Lifecycle of a node record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    /// The create job runs (see `stage`).
    Creating,
    /// Joined; a mesh peer.
    Ready,
    /// The create job stopped (see `error`); destroy cleans up.
    Failed,
    /// A restart / resize / update is in progress.
    Busy,
    /// Destroy in progress.
    Destroying,
    /// Found at the provider by its tags, with no record on this engine (made elsewhere, or the
    /// record was lost). Destroy works on it.
    Orphan,
    /// Destroyed with its disk kept (provider still bills the disk).
    VolumeKept,
}

/// Mesh path to the node (from `mesh_status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeshPath {
    /// A direct connection (the node keeps its public IPv4 for this).
    Direct,
    /// Through the iroh relay.
    Relay,
    /// Not connected.
    Offline,
}

/// One cloud node (a row in Settings › Machines).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CloudNode {
    /// Node name (also its mesh name).
    pub name: String,
    /// Where it runs.
    pub provider: ProviderKind,
    /// Region slug (`None` for Kubernetes).
    pub region: Option<String>,
    /// "London".
    pub region_label: Option<String>,
    /// 2, 4 or 8.
    pub memory_gb: Option<u8>,
    /// The provider's size slug.
    pub size_slug: Option<String>,
    /// The `/data` disk.
    pub volume_gb: Option<u32>,
    /// Server + disk per month from the provider's live prices (a kept disk alone); `None` when
    /// unknown, and always in events.
    pub monthly_usd: Option<f64>,
    /// Lifecycle.
    pub state: NodeState,
    /// While `Creating`: the current stage.
    pub stage: Option<Stage>,
    /// Why the create job or the last operation failed.
    pub error: Option<String>,
    /// The provider's own word (`active`, `off`, `started`, `Running`).
    pub provider_state: Option<String>,
    /// Public IPv4 (no port is open on it).
    pub ip: Option<String>,
    /// Its mesh endpoint id once joined.
    pub peer_id: Option<String>,
    /// A mesh connection is up.
    pub reachable: Option<bool>,
    /// How the mesh reaches it.
    pub path: Option<MeshPath>,
    /// `t --version` on the node, when it answered.
    pub version: Option<String>,
    /// Follows stable releases (always true for nodes this builds).
    pub auto_update: bool,
    /// Created with a watchable desktop (`cloud_desktop`).
    pub desktop: bool,
    /// When this engine created it (Unix ms).
    pub created_ms: Option<u64>,
    /// Kubernetes: where it runs.
    pub k8s: Option<K8sOptions>,
    /// How its vault is protected; `None` when unknown (older records, orphans).
    pub vault: Option<VaultProtection>,
    /// Phone-only nodes (224 §16.12): the node forms its own mesh and the app pairs with it over
    /// the relay. `None` for a node that joined this engine's mesh.
    #[serde(default)]
    pub link: Option<NodeLink>,
}

/// How the app reaches a node made by an engine that is not a mesh member (224 §16.12). Public
/// data only: the claim's PSK stays in this engine's vault until the app paired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NodeLink {
    /// The one-time link claim's relay rendezvous id (a hash of its PSK).
    pub claim_id: String,
    /// When the claim dies (Unix ms).
    pub expires_ms: u64,
    /// The node's link identity (base64url public key) once it answered on the relay.
    pub engine: Option<String>,
    /// The name the node answered with.
    pub engine_name: Option<String>,
    /// The app paired with the node; the claim is spent and forgotten here.
    #[serde(default)]
    pub paired: bool,
}

/// `cloud_list`: every node from local records and every connected provider's tagged
/// resources; a provider that fails is reported in `errors`, not fatal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CloudList {
    /// Every node.
    pub nodes: Vec<CloudNode>,
    /// Providers that could not be listed.
    pub errors: Vec<ProviderError>,
}

/// A provider `cloud_list` could not ask.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderError {
    /// Which provider.
    pub provider: ProviderKind,
    /// What went wrong.
    pub message: String,
}

/// `cloud_k8s_info`: what the Kubernetes variant of step 2 needs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct K8sInfo {
    /// Contexts in the kubeconfig.
    pub contexts: Vec<String>,
    /// The kubeconfig's current context.
    pub current_context: Option<String>,
    /// Namespaces in the chosen context (empty when the user may not list them).
    pub namespaces: Vec<String>,
    /// `(name, provisioner, is_default)`.
    pub storage_classes: Vec<StorageClass>,
    /// No class is marked default (a PVC without a class stays Pending).
    pub no_default_class: bool,
    /// The CNI detected from kube-system pods/daemonsets (`flannel`, `calico`, `cilium`, …).
    pub cni: Option<String>,
    /// `false` for a CNI known not to enforce NetworkPolicy (flannel); `None` when unknown.
    pub enforces_network_policy: Option<bool>,
}

/// A StorageClass of the cluster.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StorageClass {
    /// Its name.
    pub name: String,
    /// `provisioner`.
    pub provisioner: String,
    /// Marked as the cluster's default.
    pub default: bool,
}

/// `cloud_oauth_start`: open `url` in the system browser; completion arrives as
/// [`CloudEvent::CloudCredentialChanged`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OauthStart {
    /// theuth-relay's start URL (no secret in it).
    pub url: String,
}

/// `cloud_terminal`: an in-memory login shell on the node's engine (no session, no job); attach
/// with `subscribe_shell(shell, rows, cols, engine)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalOpen {
    /// Shell id in the node engine's in-memory shell registry (default `"main"`).
    pub shell: String,
    /// The node's mesh endpoint id (where the shell runs).
    pub engine: String,
}

/// `cloud_desktop`: a noVNC endpoint on this engine that tunnels over the mesh to the node's
/// loopback websockify, and its password. An owner-only RPC answer, never in an event (as
/// `sandbox_desktop_access`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DesktopOpen {
    /// `ws://127.0.0.1:<port>/…` on the viewing engine.
    pub ws_url: String,
    /// The VNC password (RPC answer only).
    pub password: String,
    /// `1280x800`, when the node says.
    pub geometry: Option<String>,
    /// The node's mesh endpoint id.
    pub engine: String,
}

/// `cloud_logs`: the tail of the node engine's own log, read over the mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NodeLogs {
    /// Node name.
    pub name: String,
    /// Oldest first, redacted by the node.
    pub lines: Vec<String>,
}

/// Bus events on `cloudnodes.events` (variant tags unique across topics).
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum CloudEvent {
    /// The whole row, after any change.
    CloudNodeChanged {
        /// The row.
        node: CloudNode,
    },
    /// One step of a long job (create, resize, destroy): the stage it belongs to and a line.
    CloudNodeProgress {
        /// Node name.
        name: String,
        /// The create job's stage (`None` for other jobs).
        stage: Option<Stage>,
        /// What happens now.
        line: String,
        /// 0-100, the create job's estimate.
        pct: Option<u8>,
        /// Seconds left, the create job's estimate ("About 2 minutes left").
        eta_secs: Option<u32>,
    },
    /// A node's record is gone (destroyed).
    CloudNodeRemoved {
        /// Node name.
        name: String,
    },
    /// A credential was stored or removed, or an OAuth sign-in finished (`error` when it failed).
    CloudCredentialChanged {
        /// Which provider.
        provider: ProviderKind,
        /// A credential is stored now.
        connected: bool,
        /// Why a sign-in failed.
        error: Option<String>,
    },
}

impl wire::WireEvent for CloudEvent {
    const TOPIC: &'static str = "cloudnodes.events";
}
