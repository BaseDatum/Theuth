//! Update plugin: check the release manifest, verify, replace `theuth` on disk;
//! never ask (design doc 39).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`UPDATE_ACTOR`] actor ([`UpdateActor`]): [`Status`]
//! - events: [`UpdateReady`]
//! - [`run_once`] for `t --update`

/// Settings RPC methods and typed clients.
pub mod rpc_settings;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::merge_described([
        rpc_settings::rpc_module(server),
    ]).expect("update RPC composition")
}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use base64::Engine as _;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use minisign_verify::{PublicKey, Signature};
use runtime::Runtime;
use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;
use tokio::time::Instant;

/// Registry name of the [`UpdateActor`].
pub const UPDATE_ACTOR: &str = "update";

/// Primary release manifest on GitHub Releases (39 §3.1).
pub const GITHUB_URL: &str =
    "https://github.com/BaseDatum/Theuth/releases/latest/download/latest.json";

/// Secondary release manifest on Wasabi S3 (39 §3.1).
pub const WASABI_URL: &str = "https://s3.us-central-1.wasabisys.com/theuth-releases/latest.json";

/// Tertiary release manifest on self-hosted S3 (39 §3.1).
pub const S3_URL: &str = "https://s3.rwx.dev/theuth/releases/latest.json";

/// Default release manifest endpoints, tried in order (GitHub Releases -> Wasabi S3 -> self-hosted S3).
pub const DEFAULT_URLS: &[&str] = &[GITHUB_URL, WASABI_URL, S3_URL];

/// Primary manifest URL (39 §3.1). Kept for callers expecting `update::URL`;
/// `update.url` overrides the default fallback chain.
pub const URL: &str = GITHUB_URL;

/// The minisign public key every payload is verified against (39 §5.1), in
/// Tauri's encoding (the `.pub` file, base64). Shared with the desktop's
/// `tauri.conf.json` by value; the release script checks the two match.
pub const PUBKEY: &str = include_str!("../pubkey.txt");

/// Delay before the first check after start (39 §9).
const FIRST_CHECK: Duration = Duration::from_secs(60);
/// Default interval between manifest checks (39 §9).
const EVERY: Duration = Duration::from_secs(30 * 60);
/// Upper bound of the per-tick jitter added to [`EVERY`] (39 §9).
const JITTER_SECS: u64 = 120;
/// Safe lower bound for `update.interval_secs`.
const MIN_INTERVAL_SECS: u64 = 60;
/// Cadence for checking restart readiness once an update is applied on disk on
/// a managed daemon (39 §3.4), without re-fetching the network manifest.
const IDLE_POLL: Duration = Duration::from_secs(30);
/// Connect and read timeout on every request (39 §3.2 step 1).
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// A service daemon exits for restart only after this long without a turn (39 §3.4).
const IDLE_BEFORE_EXIT: Duration = Duration::from_secs(10 * 60);
/// `brew upgrade` budget (39 §3.2 step H).
const BREW_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// `EX_TEMPFAIL`: non-zero, so launchd/systemd restart the daemon (39 §3.4).
const EXIT_RESTART: i32 = 75;

/// `update:` config section (39 §7).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// `false`: the harness never touches the network for this.
    pub enabled: bool,
    /// Optional single manifest URL override (e.g. a scratch bucket or local mock).
    /// When unset (`None` or blank), [`DEFAULT_URLS`] are tried in order.
    pub url: Option<String>,
    /// Optional override for the manifest check interval in seconds.
    /// Defaults to [`EVERY`] (30 min); clamped to a minimum of 60 s.
    #[serde(default)]
    pub interval_secs: Option<u64>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            url: None,
            interval_secs: None,
        }
    }
}

impl UpdateConfig {
    /// Manifest URLs to try in order.
    ///
    /// When `url` is unset (or blank), returns [`DEFAULT_URLS`] (GitHub Releases,
    /// then Wasabi S3, then self-hosted S3). When `url` is explicitly configured, returns only that
    /// single URL with no fallback to production endpoints.
    pub fn endpoints(&self) -> Vec<&str> {
        match self.url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(url) => vec![url],
            None => DEFAULT_URLS.to_vec(),
        }
    }

    /// Interval between manifest checks: `interval_secs` clamped to at least
    /// 60 s, or [`EVERY`] (30 min) when unset.
    pub fn check_interval(&self) -> Duration {
        match self.interval_secs {
            Some(secs) => Duration::from_secs(secs.max(MIN_INTERVAL_SECS)),
            None => EVERY,
        }
    }
}

/// Event: a newer version was verified and is on disk (`applied: true`), or
/// exists but the binary's directory is not writable (`applied: false`)
/// (39 §3.2). Published once per version.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct UpdateReady {
    /// The version that is ready (`0.2.0`).
    pub version: String,
    /// `true`: the next start of `theuth` runs it. `false`: `t --update`
    /// (with `sudo`) installs it.
    pub applied: bool,
}

impl wire::WireEvent for UpdateReady {
    const TOPIC: &'static str = "update.ready";
    const VERSION: u32 = 1;
}

/// Declare wire contracts before starting producers, including disabled services.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<UpdateReady>().await
}

/// `ask` → `Option<UpdateReady>`: the latest event, if any (39 §4.3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status;

/// Outcome of asking [`UpdateActor`] whether a managed daemon should exit for
/// restart now or keep polling on [`IDLE_POLL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, kameo::Reply)]
enum RestartDecision {
    /// Ready to drain and exit with [`EXIT_RESTART`].
    Restart,
    /// Applied update is pending on a managed daemon, waiting for the
    /// idle/unattached window.
    WaitIdle,
    /// Nothing applied, or not under a service manager.
    None,
}

/// Deduplication key for the `INFO` log emitted while a managed daemon waits
/// for the restart window (changes only on version or state transition, never
/// on the ticking idle countdown).
#[derive(Clone, Debug, PartialEq, Eq)]
enum WaitReason {
    UnknownAttached,
    Blocked {
        attached: usize,
        active_turns: usize,
        idle_pending: bool,
    },
}

/// `ask` (internal, from the tick task) → [`RestartDecision`]: carries what a
/// manifest check found; the actor publishes it when it is news, then checks
/// restart readiness (39 §3.4).
struct Landed(Option<UpdateReady>);

/// `ask` (internal, from the tick task on [`IDLE_POLL`]) → [`RestartDecision`]:
/// re-checks restart readiness without touching the network.
struct ShouldRestart;

/// The [`UPDATE_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct UpdateActor {
    rt: Runtime,
    ready: Option<UpdateReady>,
    attached: Option<Arc<dyn Fn() -> usize + Send + Sync>>,
    managed: Arc<dyn Fn() -> bool + Send + Sync>,
    /// When the last turn started or ended; `now` at spawn so a fresh daemon
    /// never exits inside its first [`IDLE_BEFORE_EXIT`].
    last_turn: Instant,
    /// In-flight turns keyed by `(session, turn)`.
    active_turns: HashSet<(runtime::SessionId, runtime::TurnId)>,
    /// Last `(version, reason)` logged at `INFO` while waiting to restart.
    waiting_logged: Option<(String, WaitReason)>,
}

impl UpdateActor {
    fn evaluate_restart(&mut self) -> RestartDecision {
        // 39 §3.4: (a) under a service manager, (b) nobody attached,
        // (c) no turn in flight and idle for IDLE_BEFORE_EXIT.
        let Some(ready) = self.ready.as_ref().filter(|r| r.applied) else {
            self.waiting_logged = None;
            return RestartDecision::None;
        };
        if !(self.managed)() {
            self.waiting_logged = None;
            return RestartDecision::None;
        }
        let attached = self.attached.as_ref().map(|f| f());
        let active_turns = self.active_turns.len();
        let idle_remaining = IDLE_BEFORE_EXIT.saturating_sub(self.last_turn.elapsed());
        if attached == Some(0) && active_turns == 0 && idle_remaining.is_zero() {
            self.waiting_logged = None;
            tracing::info!("update: {} in place, exiting for restart", ready.version);
            return RestartDecision::Restart;
        }
        let reason = match attached {
            Some(attached) => WaitReason::Blocked {
                attached,
                active_turns,
                idle_pending: !idle_remaining.is_zero(),
            },
            None => WaitReason::UnknownAttached,
        };
        let key = (ready.version.clone(), reason);
        if self.waiting_logged.as_ref() != Some(&key) {
            match attached {
                Some(count) => tracing::info!(
                    "update: {} applied, waiting for restart window (attached={count}, active_turns={active_turns}, idle_remaining={}s)",
                    ready.version,
                    idle_remaining.as_secs(),
                ),
                None => tracing::info!(
                    "update: {} applied, waiting for restart window (attached=unknown, active_turns={active_turns}, idle_remaining={}s)",
                    ready.version,
                    idle_remaining.as_secs(),
                ),
            }
            self.waiting_logged = Some(key);
        }
        RestartDecision::WaitIdle
    }
}

impl Message<Status> for UpdateActor {
    type Reply = Option<UpdateReady>;
    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ready.clone()
    }
}

impl Message<agent::AgentEvent> for UpdateActor {
    type Reply = ();
    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        match ev {
            agent::AgentEvent::TurnStarted { session, turn, .. }
            | agent::AgentEvent::TurnResumed { session, turn, .. } => {
                self.active_turns.insert((session, turn));
                self.last_turn = Instant::now();
            }
            agent::AgentEvent::TurnFinished { session, turn, .. }
            | agent::AgentEvent::TurnSuspended { session, turn, .. } => {
                self.active_turns.remove(&(session, turn));
                self.last_turn = Instant::now();
            }
            agent::AgentEvent::SessionDeleted { session }
            | agent::AgentEvent::SessionClosing { session, .. }
            | agent::AgentEvent::SessionOwnerChanged {
                session,
                here: false,
                ..
            } => {
                let before = self.active_turns.len();
                self.active_turns.retain(|(s, _)| *s != session);
                if self.active_turns.len() != before {
                    self.last_turn = Instant::now();
                }
            }
            _ => {}
        }
    }
}

impl Message<Landed> for UpdateActor {
    type Reply = RestartDecision;
    async fn handle(&mut self, m: Landed, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(ready) = m.0
            && self.ready.as_ref() != Some(&ready)
        {
            self.rt.publish(ready.clone()).await;
            self.ready = Some(ready);
        }
        self.evaluate_restart()
    }
}

impl Message<ShouldRestart> for UpdateActor {
    type Reply = RestartDecision;
    async fn handle(&mut self, _: ShouldRestart, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.evaluate_restart()
    }
}

/// What one foreground check did (`t --update`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The manifest names nothing newer for this platform.
    UpToDate,
    /// The binary on disk is now `to`; the running process is still `from`.
    Applied {
        /// Running version.
        from: String,
        /// Installed version.
        to: String,
    },
    /// Newer exists but `path` is not writable by this user.
    Available {
        /// The version in the manifest.
        version: String,
        /// The file that would have been replaced.
        path: PathBuf,
    },
}

impl Outcome {
    fn ready(self) -> Option<UpdateReady> {
        match self {
            Self::UpToDate => None,
            Self::Applied { to, .. } => Some(UpdateReady {
                version: to,
                applied: true,
            }),
            Self::Available { version, .. } => Some(UpdateReady {
                version,
                applied: false,
            }),
        }
    }
}

/// `latest.json` (39 §3.1); keys this binary does not use are ignored.
#[derive(Deserialize)]
struct Manifest {
    version: String,
    platforms: HashMap<String, Platform>,
}

/// One `platforms` entry: the tarball and its `.sig` file contents.
#[derive(Deserialize)]
struct Platform {
    url: String,
    signature: String,
}

/// 39 §1.7: newer, same major.
fn newer(current: &Version, remote: &Version) -> bool {
    remote > current && remote.major == current.major
}

fn current_version() -> anyhow::Result<Version> {
    Version::parse(platform::version::get()).context("the product version is not semver")
}

/// Tauri's encoding: base64 of the minisign text file (`.pub` / `.sig`).
fn tauri_b64(s: &str) -> anyhow::Result<String> {
    let bin = base64::prelude::BASE64_STANDARD.decode(s.trim())?;
    Ok(String::from_utf8(bin)?)
}

fn pubkey_from(b64: &str) -> anyhow::Result<PublicKey> {
    let text = tauri_b64(b64).map_err(|_| anyhow::anyhow!("update: pubkey not configured"))?;
    PublicKey::decode(&text).map_err(|_| anyhow::anyhow!("update: pubkey not configured"))
}

/// One check-and-apply in the foreground (39 §3.2 `t --update`).
/// `data_dir` is the harness data dir; the tarball is spooled under
/// `<data_dir>/updates/`.
pub async fn run_once(cfg: &UpdateConfig, data_dir: &Path) -> anyhow::Result<Outcome> {
    check_and_apply(cfg, data_dir, &current_version()?, None).await
}

async fn fetch_manifest(client: &reqwest::Client, url: &str) -> anyhow::Result<Manifest> {
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url}"))?
        .json()
        .await
        .with_context(|| format!("GET {url}"))
}

/// 39 §3.2 steps 0–4 and H. `applied`: the version an earlier tick already
/// put in place, so a tick does not re-download what is already on disk
/// (and brew is invoked at most once per manifest version).
async fn check_and_apply(
    cfg: &UpdateConfig,
    data_dir: &Path,
    current: &Version,
    applied: Option<&str>,
) -> anyhow::Result<Outcome> {
    // Step 0: where am I?
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("current_exe")?;
    let endpoints = cfg.endpoints();
    check_and_apply_with(&endpoints, data_dir, &exe, current, applied, PUBKEY).await
}

async fn check_and_apply_with(
    endpoints: &[&str],
    data_dir: &Path,
    exe: &Path,
    current: &Version,
    applied: Option<&str>,
    pubkey_b64: &str,
) -> anyhow::Result<Outcome> {
    if endpoints.is_empty() {
        anyhow::bail!("no update endpoints configured");
    }
    let homebrew = exe.to_string_lossy().contains("/Cellar/theuth/");

    // Step 1: the manifest, trying candidate endpoints in order.
    let client = reqwest::Client::builder()
        .connect_timeout(HTTP_TIMEOUT)
        .read_timeout(HTTP_TIMEOUT)
        .user_agent(format!("theuth/{}", platform::version::get()))
        .build()?;
    let key = format!("cli-{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let mut errors: Vec<String> = Vec::new();
    let mut missing_platform = false;

    for (idx, &endpoint) in endpoints.iter().enumerate() {
        let has_next = idx + 1 < endpoints.len();
        let manifest = match fetch_manifest(&client, endpoint).await {
            Ok(m) => m,
            Err(e) => {
                if has_next {
                    tracing::debug!("update: manifest {endpoint} failed ({e:#}); trying next endpoint");
                } else {
                    tracing::debug!("update: manifest {endpoint} failed: {e:#}");
                }
                errors.push(format!("{endpoint}: {e:#}"));
                continue;
            }
        };
        let remote = match Version::parse(&manifest.version)
            .with_context(|| format!("manifest version {:?}", manifest.version))
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("update: manifest {endpoint} invalid version: {e:#}");
                errors.push(format!("{endpoint}: {e:#}"));
                continue;
            }
        };
        if !newer(current, &remote) {
            return Ok(Outcome::UpToDate);
        }
        let version = manifest.version;
        let Some(platform) = manifest.platforms.get(&key) else {
            if has_next {
                tracing::warn!(
                    "update: {version} from {endpoint} has no `{key}` entry; trying next endpoint"
                );
            } else {
                tracing::debug!("update: {version} from {endpoint} has no `{key}` entry");
            }
            missing_platform = true;
            continue;
        };
        let done = Outcome::Applied {
            from: current.to_string(),
            to: version.clone(),
        };
        if applied == Some(version.as_str()) {
            return Ok(done);
        }

        // Step H: the package manager's file is the package manager's.
        if homebrew {
            brew_upgrade(&version).await?;
            tracing::info!("update: applied {current} -> {version}");
            return Ok(done);
        }

        // Step 4: can we replace it?
        let dir = exe.parent().context("current_exe has no parent")?;
        if !writable(dir) {
            tracing::info!(
                "update: {version} is available but {} is not writable; `sudo t --update` installs it",
                exe.display()
            );
            return Ok(Outcome::Available {
                version,
                path: exe.to_path_buf(),
            });
        }

        // Steps 2–3: download + verify, extract, rename.
        let updates = data_dir.join("updates");
        tokio::fs::create_dir_all(&updates)
            .await
            .with_context(|| updates.display().to_string())?;
        // Private name: another updater (other process or this one's foreground
        // `--update` beside the tick loop) must never truncate the tarball this
        // one verified and is about to unpack.
        let tarball = updates.join(format!(
            "theuth_{version}.{:016x}.tar.gz.part",
            rand::random::<u64>()
        ));
        match download(
            &client,
            &platform.url,
            &platform.signature,
            &tarball,
            pubkey_b64,
        )
        .await
        {
            Ok(()) => {
                let result = install(&tarball, exe).await;
                let _ = std::fs::remove_file(&tarball);
                result?;
                tracing::info!("update: applied {current} -> {version}");
                return Ok(done);
            }
            Err(DownloadError::Transport(e)) => {
                let _ = std::fs::remove_file(&tarball);
                if has_next {
                    tracing::warn!(
                        "update: download {} (from {endpoint}) failed ({e:#}); trying next endpoint",
                        platform.url
                    );
                } else {
                    tracing::warn!(
                        "update: download {} (from {endpoint}) failed: {e:#}",
                        platform.url
                    );
                }
                errors.push(format!("{endpoint} (payload {}): {e:#}", platform.url));
            }
            Err(DownloadError::Fatal(e)) => {
                let _ = std::fs::remove_file(&tarball);
                return Err(e);
            }
        }
    }

    if errors.is_empty() && missing_platform {
        return Ok(Outcome::UpToDate);
    }
    if errors.len() == 1 {
        anyhow::bail!("update failed: {}", errors[0]);
    }
    anyhow::bail!("all update endpoints failed: {}", errors.join("; "));
}

/// Step H: `brew upgrade --formula theuth`, then confirm the Cellar holds `version`.
async fn brew_upgrade(version: &str) -> anyhow::Result<()> {
    let out = tokio::time::timeout(
        BREW_TIMEOUT,
        tokio::process::Command::new("brew")
            .args(["upgrade", "--formula", "theuth"])
            .env("HOMEBREW_AUTO_UPDATE_SECS", "0")
            .env("HOMEBREW_NO_ENV_HINTS", "1")
            .env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
            .output(),
    )
    .await
    .context("brew upgrade: timed out")?
    .context("brew upgrade")?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    tracing::debug!(
        "update: brew upgrade: {}\n{}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout).trim(),
        stderr.trim()
    );
    if !out.status.success() {
        anyhow::bail!(
            "brew upgrade: {}",
            stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
        );
    }
    let out = tokio::process::Command::new("brew")
        .args(["list", "--versions", "theuth"])
        .output()
        .await
        .context("brew list")?;
    let listed = String::from_utf8_lossy(&out.stdout);
    if !listed.split_whitespace().any(|v| v == version) {
        anyhow::bail!(
            "brew upgrade ran but `brew list --versions theuth` says `{}`",
            listed.trim()
        );
    }
    Ok(())
}

/// Step 4 probe: can this user create a file in `dir`?
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".theuth-update-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

enum DownloadError {
    /// HTTP/network failure fetching the payload — eligible for endpoint fallback.
    Transport(anyhow::Error),
    /// Signature verification, pubkey, or local filesystem failure — fatal.
    Fatal(anyhow::Error),
}

/// Step 2: stream `url` to `dest`, hashing as it lands; verify against
/// `signature` (Tauri-encoded `.sig`) before returning. `dest` is left in
/// place on error for the caller to remove.
async fn download(
    client: &reqwest::Client,
    url: &str,
    signature: &str,
    dest: &Path,
    pubkey_b64: &str,
) -> Result<(), DownloadError> {
    let pk = pubkey_from(pubkey_b64).map_err(DownloadError::Fatal)?;
    let sig = Signature::decode(
        &tauri_b64(signature)
            .context("signature encoding")
            .map_err(DownloadError::Fatal)?,
    )
    .context("signature")
    .map_err(DownloadError::Fatal)?;
    let mut verifier = pk
        .verify_stream(&sig)
        .context("signature")
        .map_err(DownloadError::Fatal)?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))
        .map_err(DownloadError::Transport)?
        .error_for_status()
        .with_context(|| format!("GET {url}"))
        .map_err(DownloadError::Transport)?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| dest.display().to_string())
        .map_err(DownloadError::Fatal)?;
    while let Some(chunk) = resp
        .chunk()
        .await
        .with_context(|| format!("read {url}"))
        .map_err(DownloadError::Transport)?
    {
        verifier.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| {
            DownloadError::Fatal(anyhow::Error::new(e).context(dest.display().to_string()))
        })?;
    }
    file.flush().await.map_err(|e| {
        DownloadError::Fatal(anyhow::Error::new(e).context(dest.display().to_string()))
    })?;
    drop(file);
    verifier
        .finalize()
        .context("signature does not verify")
        .map_err(DownloadError::Fatal)?;
    Ok(())
}

/// Prefix of the app parser helper beside `theuth` (46 §2.1): the file is
/// `theuth-app-parser-<library sha256 hex>`, immutable per hash.
const HELPER_PREFIX: &str = "theuth-app-parser-";
/// Prefix of the documents helper beside `theuth` (59 §3.2): the file is
/// `theuth-documents-<protocol version>`, immutable per version.
const DOC_HELPER_PREFIX: &str = "theuth-documents-";
/// Upper bound on one tarball entry. A release `theuth` is ~330 MB today, the
/// helper ~60 MiB; the bound only stops a runaway header, not a real build.
const MAX_ENTRY: u64 = 2 << 30;
/// `theuth.new --app-parser-hash` budget: the probe answers from a constant.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// `theuth-app-parser-<64 lowercase hex>`.
fn is_helper_name(name: &str) -> bool {
    name.strip_prefix(HELPER_PREFIX).is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// `theuth-documents-<digits and dots>`.
fn is_doc_helper_name(name: &str) -> bool {
    name.strip_prefix(DOC_HELPER_PREFIX)
        .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}

/// Step 3: the payload holds `theuth`, at most one `theuth-app-parser-<hash>` and at most one
/// `theuth-documents-<version>` (regular files, nothing else). All land beside `exe` as `<name>.new.<pid>-<nonce>`,
/// `chmod 755`; when the payload ships an app-parser entry, the new `theuth` is asked which
/// hash it was compiled against and must name the one shipped beside it (the entry is a
/// compatibility stub since the in-process parser, apps-inproc-as-built-P1.md, and optional so a
/// later release can drop it); then swap: helper first, then
/// `theuth`, so a `theuth` that exists always has its helper. Same directory, so each
/// swap is an atomic rename and the running image is untouched (39 §2.2 `ETXTBSY`);
/// on Windows the running `theuth.exe` goes through `self-replace` (58 §0.5).
/// The staging names are private to this install (created with `create_new`),
/// so two updaters aimed at one `theuth` never probe, rename, or delete each
/// other's files: the pair that lands is always the pair that was probed. A
/// helper already installed under the payload's hash is kept, not replaced:
/// the name is the library hash, one ABI contract per name, and a running
/// `theuth` may be spawning it. Old helpers are never removed here: any number of
/// older `theuth` processes may still spawn theirs (46 §2.1).
// ponytail: helpers accumulate one file per library hash; sweep them from a
// place that knows every live `theuth` (a `t --gc-helpers` on the newest
// binary) if the directory ever gets noisy.
async fn install(tarball: &Path, exe: &Path) -> anyhow::Result<()> {
    let dir = exe.parent().context("current_exe has no parent")?;
    let tag = format!(".new.{}-{:016x}", std::process::id(), rand::random::<u64>());
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = match unpack(tarball, exe, dir, &tag, &mut staged) {
        // `unpack` sorts `t` last.
        Ok(hash) => match &hash {
            Some(hash) => probe(&staged[staged.len() - 1].0, hash).await,
            None => Ok(()),
        }
        .and_then(|()| {
                for (new, dest) in &staged {
                    if dest != exe && dest.exists() {
                        let _ = std::fs::remove_file(new);
                        continue;
                    }
                    platform::files::replace_executable(new, dest)
                        .with_context(|| format!("replace {}", dest.display()))?;
                }
                Ok(())
            }),
        Err(e) => Err(e),
    };
    if result.is_err() {
        // Only files this install created: never another updater's stage.
        for (new, _) in &staged {
            let _ = std::fs::remove_file(new);
        }
    }
    result
}

/// Run the signature-verified new binary once: `--app-parser-hash` prints the
/// library hash it embeds, which must be the helper in the same payload; a
/// `theuth` that could never find its helper is not installed.
async fn probe(new_theuth: &Path, hash: &str) -> anyhow::Result<()> {
    let mut cmd = tokio::process::Command::new(new_theuth);
    cmd.arg("--app-parser-hash")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    platform::process::hide_console(cmd.as_std_mut());
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
        .await
        .context("probe `t.new --app-parser-hash`: timed out")?
        .context("probe `t.new --app-parser-hash`")?;
    let got = String::from_utf8_lossy(&out.stdout);
    let got = got.trim();
    if !out.status.success() {
        anyhow::bail!(
            "probe `t.new --app-parser-hash`: {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if got != hash {
        anyhow::bail!("payload helper is {HELPER_PREFIX}{hash} but the new t wants {got:?}");
    }
    Ok(())
}

/// Extract the validated entries to `<dest><tag>`, each created fresh
/// (`create_new`) so the path is provably this install's; `staged` is filled in
/// commit order (helpers, then `theuth`), including on error so the caller can
/// clean up. Returns the app parser entry's hash, if the payload has one.
fn unpack(
    tarball: &Path,
    exe: &Path,
    dir: &Path,
    tag: &str,
    staged: &mut Vec<(PathBuf, PathBuf)>,
) -> anyhow::Result<Option<String>> {
    let file = std::fs::File::open(tarball).with_context(|| tarball.display().to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut theuth = false;
    let mut helper: Option<String> = None;
    let mut doc_helper = false;
    for entry in archive.entries().context("tarball")? {
        let mut entry = entry.context("tarball entry")?;
        let path = entry.path().context("tarball entry path")?;
        let mut components = path.components();
        let name = match (components.next(), components.next()) {
            (Some(std::path::Component::Normal(n)), None) => n.to_str(),
            _ => None,
        }
        .with_context(|| format!("tarball entry {path:?}: not a bare file name"))?
        .to_owned();
        if entry.header().entry_type() != tar::EntryType::Regular {
            anyhow::bail!("tarball entry `{name}`: not a regular file");
        }
        let size = entry.header().size().context("tarball entry size")?;
        if size > MAX_ENTRY {
            anyhow::bail!("tarball entry `{name}`: {size} bytes exceeds {MAX_ENTRY}");
        }
        let (dup, dest) = if name == "t" {
            (std::mem::replace(&mut theuth, true), exe.to_path_buf())
        } else if is_helper_name(&name) {
            (
                helper
                    .replace(name[HELPER_PREFIX.len()..].to_owned())
                    .is_some(),
                dir.join(&name),
            )
        } else if is_doc_helper_name(&name) {
            (std::mem::replace(&mut doc_helper, true), dir.join(&name))
        } else {
            anyhow::bail!("tarball entry `{name}`: unexpected");
        };
        if dup {
            anyhow::bail!("tarball entry `{name}`: duplicate");
        }
        let new = dir.join(format!("{name}{tag}"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new)
            .with_context(|| new.display().to_string())?;
        staged.push((new.clone(), dest));
        std::io::copy(&mut entry, &mut file).with_context(|| new.display().to_string())?;
        drop(file);
        platform::files::set_executable(&new).with_context(|| new.display().to_string())?;
    }
    if !theuth {
        anyhow::bail!("tarball has no `t` entry");
    }
    // Commit order: helper first, `t` last.
    staged.sort_by_key(|(_, dest)| dest == exe);
    Ok(helper)
}

/// The plugin (39 §3.2): registers [`UpdateActor`] as [`UPDATE_ACTOR`]; runs
/// the tick loop only when this process owns its binary and is a release build.
pub struct UpdatePlugin;

#[async_trait::async_trait]
impl runtime::Plugin for UpdatePlugin {
    fn name(&self) -> &'static str {
        "update"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: UpdateConfig = rt.section("update").unwrap_or_else(|e| {
            tracing::debug!("no [update] config section, using defaults: {e}");
            UpdateConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("update disabled by config");
            return Ok(());
        }
        use wire::RuntimeWire as _;
        let feed_rt = rt.clone();
        let actor = UpdateActor::spawn_with_mailbox(
            UpdateActor {
                rt: rt.clone(),
                ready: None,
                attached: Some(Arc::new(move || feed_rt.wire_external_stream_count())),
                managed: Arc::new(platform::service::managed),
                last_turn: Instant::now(),
                active_turns: HashSet::new(),
                waiting_logged: None,
            },
            runtime::subscriber_mailbox(),
        );
        actor.register(UPDATE_ACTOR)?;
        // The actor exists so `update_status` answers `null` even when no
        // loop runs.
        if cfg!(debug_assertions) {
            tracing::debug!("update: debug build, not checking");
            return Ok(());
        }
        let bundled = std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"));
        if bundled {
            tracing::debug!("update: bundled sidecar, the app owns it");
            return Ok(());
        }
        let current = match current_version() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("update unavailable: {e:#}");
                return Ok(());
            }
        };
        rt.subscribe::<UpdateActor, agent::AgentEvent>(&actor).await;
        spawn_ticks(rt.clone(), actor, cfg, current);
        Ok(())
    }
}

/// Core tick loop (39 §3.2, §3.4): checks the manifest at [`FIRST_CHECK`], then
/// every `interval + rand(0..jitter_secs)`. Once an applied update is pending
/// on a managed daemon (`RestartDecision::WaitIdle`), polls [`ShouldRestart`]
/// every [`IDLE_POLL`] without re-fetching the network manifest until
/// `next_manifest_check` arrives or the daemon becomes ready to restart.
///
/// Returns `true` when the daemon should drain and exit with [`EXIT_RESTART`],
/// or `false` if shutdown was signalled or the actor stopped.
async fn run_tick_loop<F, Fut>(
    rt: Runtime,
    me: ActorRef<UpdateActor>,
    interval: Duration,
    jitter_secs: u64,
    mut check: F,
) -> bool
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Outcome>>,
{
    let mut next_manifest_check = Instant::now() + FIRST_CHECK;
    let mut decision = RestartDecision::None;
    loop {
        let next_wake = if decision == RestartDecision::WaitIdle {
            next_manifest_check.min(Instant::now() + IDLE_POLL)
        } else {
            next_manifest_check
        };
        tokio::select! {
            () = rt.shutdown.cancelled() => return false,
            () = tokio::time::sleep_until(next_wake) => {}
        }
        let next = if Instant::now() >= next_manifest_check {
            let applied = me
                .ask(Status)
                .await
                .ok()
                .flatten()
                .filter(|r| r.applied)
                .map(|r| r.version);
            let found = match check(applied).await {
                Ok(outcome) => outcome.ready(),
                Err(e) => {
                    tracing::warn!("update: {e:#}");
                    None
                }
            };
            let jitter = if jitter_secs > 0 {
                Duration::from_secs(rand::random_range(0..jitter_secs))
            } else {
                Duration::ZERO
            };
            next_manifest_check = Instant::now() + interval + jitter;
            me.ask(Landed(found)).await.ok()
        } else {
            me.ask(ShouldRestart).await.ok()
        };
        match next {
            Some(RestartDecision::Restart) => return true,
            Some(d) => decision = d,
            None => return false,
        }
    }
}

/// The tick loop (39 §3.2): network in the task, state in the actor.
fn spawn_ticks(rt: Runtime, me: ActorRef<UpdateActor>, cfg: UpdateConfig, current: Version) {
    let interval = cfg.check_interval();
    tokio::spawn(async move {
        let rt_for_check = rt.clone();
        let rt_for_restart = rt.clone();
        let should_restart = run_tick_loop(rt, me, interval, JITTER_SECS, move |applied| {
            let cfg = cfg.clone();
            let data_dir = rt_for_check.data_dir.clone();
            let current = current.clone();
            async move { check_and_apply(&cfg, &data_dir, &current, applied.as_deref()).await }
        })
        .await;
        if should_restart {
            // 109 §9.3: a drain precedes the swap; the token then
            // lets the jobs actor kill its groups as on any exit.
            agent::supervisor::drain_engine(&rt_for_restart, "update").await;
            rt_for_restart.shutdown.cancel();
            tokio::time::sleep(Duration::from_millis(100)).await;
            std::process::exit(EXIT_RESTART)
        }
    });
}

pub mod rpc_support;
#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn manifest_parses_and_ignores_extras() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"0.2.0","notes":"x","platforms":{
                "darwin-aarch64":{"url":"u","signature":"s"},
                "cli-linux-x86_64":{"url":"u2","signature":"s2","extra":1}}}"#,
        )
        .unwrap();
        assert_eq!(m.version, "0.2.0");
        assert_eq!(m.platforms["cli-linux-x86_64"].url, "u2");
    }

    #[test]
    fn newer_same_major_only() {
        assert!(newer(&v("0.1.0"), &v("0.2.0")));
        assert!(!newer(&v("0.1.0"), &v("0.0.9")));
        assert!(!newer(&v("0.1.0"), &v("1.0.0")));
        assert!(!newer(&v("0.1.0"), &v("0.1.0")));
    }

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OLD_HASH: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    /// One tarball entry: name (written raw, so `..` and `/` reach the reader
    /// unfiltered), type, bytes, and the size the header claims.
    struct Entry<'a> {
        name: &'a str,
        kind: tar::EntryType,
        data: &'a [u8],
        claimed: Option<u64>,
    }

    fn file<'a>(name: &'a str, data: &'a [u8]) -> Entry<'a> {
        Entry {
            name,
            kind: tar::EntryType::Regular,
            data,
            claimed: None,
        }
    }

    fn typed(name: &str, kind: tar::EntryType) -> Entry<'_> {
        Entry {
            name,
            kind,
            data: b"",
            claimed: None,
        }
    }

    /// `.tar.gz` of `entries` at `<dir>/theuth_0.2.0.tar.gz.part`.
    fn tarball(dir: &Path, entries: &[Entry<'_>]) -> PathBuf {
        use std::io::Write as _;

        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for e in entries {
            let mut header = tar::Header::new_gnu();
            header.as_old_mut().name[..e.name.len()].copy_from_slice(e.name.as_bytes());
            header.set_entry_type(e.kind);
            header.set_size(e.claimed.unwrap_or(e.data.len() as u64));
            header.set_mode(0o644);
            if e.kind == tar::EntryType::Symlink {
                header.set_link_name("/etc/passwd").unwrap();
            }
            header.set_cksum();
            tar.append(&header, e.data).unwrap();
        }
        let mut gz = tar.into_inner().unwrap();
        gz.flush().unwrap();
        let path = dir.join("theuth_0.2.0.tar.gz.part");
        std::fs::write(&path, gz.finish().unwrap()).unwrap();
        path
    }

    /// A dir holding an old `theuth` and its old helper, as after a previous update.
    fn installed() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("t");
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(
            dir.path().join(format!("{HELPER_PREFIX}{OLD_HASH}")),
            b"old helper",
        )
        .unwrap();
        (dir, exe)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
        v.sort();
        v
    }

    /// A stand-in `theuth`: answers `--app-parser-hash` with `hash`.
    fn fake_theuth(hash: &str) -> Vec<u8> {
        format!("#!/bin/sh\necho {hash}\n").into_bytes()
    }

    #[tokio::test]
    async fn install_replaces_exe_and_adds_helpers_keeping_the_old_one() {
        let (dir, exe) = installed();
        let helper = format!("{HELPER_PREFIX}{HASH}");
        let old_helper = format!("{HELPER_PREFIX}{OLD_HASH}");
        let doc_helper = format!("{DOC_HELPER_PREFIX}59.5");
        let new_theuth = fake_theuth(HASH);
        let tb = tarball(
            dir.path(),
            &[
                file(&helper, b"#!/bin/sh\necho helper\n"),
                file(&doc_helper, b"docs"),
                file("t", &new_theuth),
            ],
        );

        install(&tb, &exe).await.unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), new_theuth);
        assert_eq!(
            std::fs::read(dir.path().join(&helper)).unwrap(),
            b"#!/bin/sh\necho helper\n"
        );
        assert_eq!(
            std::fs::read(dir.path().join(&doc_helper)).unwrap(),
            b"docs"
        );
        #[cfg(unix)]
        for f in [
            &exe,
            &dir.path().join(&helper),
            &dir.path().join(&doc_helper),
        ] {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(f).unwrap().permissions().mode() & 0o777,
                0o755,
                "{f:?}"
            );
        }
        assert_eq!(
            std::fs::read(dir.path().join(&old_helper)).unwrap(),
            b"old helper",
            "an older theuth may still be running"
        );
        assert_eq!(
            names(dir.path()),
            sorted(&[
                "t",
                &helper,
                &doc_helper,
                &old_helper,
                "theuth_0.2.0.tar.gz.part"
            ]),
            "no .new leftovers"
        );

        // A payload without the documents helper (pre-59) still installs.
        let tb = tarball(dir.path(), &[file(&helper, b"h2"), file("t", &new_theuth)]);
        install(&tb, &exe).await.unwrap();
        assert_eq!(
            std::fs::read(dir.path().join(&doc_helper)).unwrap(),
            b"docs"
        );

        // A payload without the app-parser entry (a release after the in-process parser drops the stub)
        // installs without the probe; every helper already beside `t` stays.
        let not_a_program = b"not a program".to_vec();
        let tb = tarball(dir.path(), &[file("t", &not_a_program)]);
        install(&tb, &exe).await.unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), not_a_program);
        assert_eq!(std::fs::read(dir.path().join(&helper)).unwrap(), b"#!/bin/sh\necho helper\n");
        assert_eq!(std::fs::read(dir.path().join(&old_helper)).unwrap(), b"old helper");
    }

    #[tokio::test]
    async fn install_rejects_bad_payloads_and_leaves_the_installation_intact() {
        use tar::EntryType::{Directory, Symlink};

        let helper = format!("{HELPER_PREFIX}{HASH}");
        let old_helper = format!("{HELPER_PREFIX}{OLD_HASH}");
        let upper = format!("{HELPER_PREFIX}{}", HASH.to_uppercase());
        let wants_old = fake_theuth(OLD_HASH);
        let cases: Vec<(&str, Vec<Entry<'_>>)> = vec![
            (
                "t compiled against another helper",
                vec![file("t", &wants_old), file(&helper, b"h")],
            ),
            (
                "t probe fails",
                vec![file("t", b"#!/bin/sh\nexit 3\n"), file(&helper, b"h")],
            ),
            (
                "t not executable",
                vec![file("t", b"not a program"), file(&helper, b"h")],
            ),
            ("no t", vec![file(&helper, b"h")]),
            (
                "unqualified helper",
                vec![file("t", b"new"), file("theuth-app-parser", b"h")],
            ),
            (
                "short hash",
                vec![file("t", b"new"), file("theuth-app-parser-abc123", b"h")],
            ),
            (
                "uppercase hash",
                vec![file("t", b"new"), file(&upper, b"h")],
            ),
            (
                "unqualified documents helper",
                vec![
                    file("t", b"new"),
                    file(&helper, b"h"),
                    file("theuth-documents", b"d"),
                ],
            ),
            (
                "documents helper with a non-version suffix",
                vec![
                    file("t", b"new"),
                    file(&helper, b"h"),
                    file("theuth-documents-59.5.new", b"d"),
                ],
            ),
            (
                "two documents helpers",
                vec![
                    file("t", b"new"),
                    file(&helper, b"h"),
                    file("theuth-documents-59.5", b"d"),
                    file("theuth-documents-59.6", b"d2"),
                ],
            ),
            (
                "two helpers",
                vec![
                    file("t", b"new"),
                    file(&helper, b"h"),
                    file(&old_helper, b"h2"),
                ],
            ),
            (
                "two t",
                vec![file("t", b"new"), file("t", b"new2"), file(&helper, b"h")],
            ),
            (
                "stray file",
                vec![
                    file("t", b"new"),
                    file(&helper, b"h"),
                    file("._t", b"applesauce"),
                ],
            ),
            (
                "traversal",
                vec![file("../theuth", b"new"), file(&helper, b"h")],
            ),
            ("absolute", vec![file("/t", b"new"), file(&helper, b"h")]),
            ("nested", vec![file("bin/t", b"new"), file(&helper, b"h")]),
            ("symlink t", vec![typed("t", Symlink), file(&helper, b"h")]),
            (
                "symlink helper",
                vec![file("t", b"new"), typed(&helper, Symlink)],
            ),
            (
                "directory",
                vec![
                    file("t", b"new"),
                    file(&helper, b"h"),
                    typed("d", Directory),
                ],
            ),
            (
                "oversized (header claims > MAX_ENTRY)",
                vec![
                    Entry {
                        name: "theuth",
                        kind: tar::EntryType::Regular,
                        data: b"new",
                        claimed: Some(MAX_ENTRY + 1),
                    },
                    file(&helper, b"h"),
                ],
            ),
        ];
        for (what, entries) in cases {
            let (dir, exe) = installed();
            let tb = tarball(dir.path(), &entries);
            let err = install(&tb, &exe).await.unwrap_err();
            assert_eq!(std::fs::read(&exe).unwrap(), b"old", "{what}: {err:#}");
            assert_eq!(
                std::fs::read(dir.path().join(&old_helper)).unwrap(),
                b"old helper",
                "{what}"
            );
            assert_eq!(
                names(dir.path()),
                sorted(&["t", &old_helper, "theuth_0.2.0.tar.gz.part"]),
                "{what}: staged files must be removed ({err:#})"
            );
        }
    }

    /// Two updaters aimed at one `theuth`, interleaved: A is parked inside its hash
    /// probe (its fake `theuth` waits for a gate file) while B fails, then another
    /// B succeeds; then A is released. Each install must probe, rename, and
    /// clean only its own stage: B's failure must not strip A's files, B's
    /// success must land B's own helper, and A must still land its pair.
    #[tokio::test]
    async fn concurrent_installs_never_touch_each_others_stage() {
        const B_HASH: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
        let (dir, exe) = installed();
        let scratch = tempfile::tempdir().unwrap();
        let gate = scratch.path().join("go");
        let helper_a = format!("{HELPER_PREFIX}{HASH}");
        let helper_b = format!("{HELPER_PREFIX}{B_HASH}");
        let old_helper = format!("{HELPER_PREFIX}{OLD_HASH}");
        let theuth_a = format!(
            "#!/bin/sh\nwhile [ ! -e '{}' ]; do sleep 0.01; done\necho {HASH}\n",
            gate.display()
        )
        .into_bytes();
        let theuth_b = fake_theuth(B_HASH);
        let theuth_b_bad = fake_theuth(OLD_HASH);
        let staged = |dir: &Path| {
            names(dir)
                .into_iter()
                .filter(|n| n.contains(".new."))
                .count()
        };

        let tb_a = tarball(
            scratch.path(),
            &[file(&helper_a, b"helper a"), file("t", &theuth_a)],
        );
        let a = tokio::spawn({
            let exe = exe.clone();
            async move { install(&tb_a, &exe).await }
        });
        while staged(dir.path()) < 2 {
            assert!(!a.is_finished(), "A cannot finish before the gate opens");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // B fails its probe while A is staged: A's files survive.
        let tb = tarball(
            scratch.path(),
            &[file(&helper_b, b"helper b"), file("t", &theuth_b_bad)],
        );
        install(&tb, &exe).await.unwrap_err();
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert_eq!(staged(dir.path()), 2, "B's cleanup removed A's stage");
        assert!(!dir.path().join(&helper_b).exists());

        // B succeeds while A is staged: B's own theuth and helper land.
        let tb = tarball(
            scratch.path(),
            &[file(&helper_b, b"helper b"), file("t", &theuth_b)],
        );
        install(&tb, &exe).await.unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), theuth_b);
        assert_eq!(
            std::fs::read(dir.path().join(&helper_b)).unwrap(),
            b"helper b"
        );
        assert_eq!(staged(dir.path()), 2, "B's rename consumed A's stage");
        assert!(!a.is_finished());

        // A is released: the probed theuth A lands with helper A.
        std::fs::write(&gate, b"").unwrap();
        a.await.unwrap().unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), theuth_a);
        assert_eq!(
            std::fs::read(dir.path().join(&helper_a)).unwrap(),
            b"helper a"
        );
        assert_eq!(
            std::fs::read(dir.path().join(&helper_b)).unwrap(),
            b"helper b"
        );
        assert_eq!(
            names(dir.path()),
            sorted(&["t", &helper_a, &helper_b, &old_helper]),
            "no .new leftovers"
        );
    }

    /// Ephemeral test-only minisign keypair (secret key discarded) signing a
    /// 115-byte `.tar.gz` containing one regular file `t` = `b"new-theuth-binary"`.
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXkgQURCQTVEOTQyQ0QwNzZFNApSV1RrZHRBc2xGMjZyZnliM1d3bmZJYUlsU1AzcTZzWDFRNzRCVmY2eWU0SFE3bEl2eDNsdnZzYQo=";
    const TEST_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIG1pbmlzaWduIHNlY3JldCBrZXkKUlVUa2R0QXNsRjI2cmE3MXg1bERJVXgzRDB2MXFHTGpSQk8za1VEME9OMnlHSm50ek40NTVXRlJhR2pLK2JWdW9EK0NUMUNLSGVnZFlrM1dsK3Z0Z05hVUMrRmFBTy8zRGd3PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1NjkzNjEwCWZpbGU6cGF5bG9hZC50YXIuZ3oJaGFzaGVkCkgwdk1DQkQwNEhYZEtXS1MySnBkdTZTeFRxTm95TThZQm90NURyZDJGZG9HdXY5enREMFNKQ1IyQ1MxWUlSK0w4blQ1YUF2TzlRbE1ycWVuZnBNQUJ3PT0K";
    const TEST_TGZ_B64: &str = "H4sIAAAAAAAA/+3OMQqDQBAF0D3KXkAYA+p5FATTWJiV4O0Vm0CqNDEp3mv+8Jv5JX1fHLqmOfPwnhG3+nWffRttl3JcsC2tj9IvOV/x6h/N47Mq07iWqRruc79svx4EAAAAAAAAAADAR3Z4WsXXACgAAA==";
    const TEST_BINARY: &[u8] = b"new-theuth-binary";

    fn test_tgz() -> Vec<u8> {
        base64::prelude::BASE64_STANDARD
            .decode(TEST_TGZ_B64)
            .unwrap()
    }

    fn host_platform_key() -> String {
        format!("cli-{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    }

    struct MockServer {
        base_url: String,
        hits: Arc<std::sync::atomic::AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl MockServer {
        fn hits(&self) -> usize {
            self.hits.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base_url)
        }
    }

    async fn spawn_mock_server(
        handler: impl Fn(&str) -> (u16, Vec<u8>) + Send + Sync + 'static,
    ) -> MockServer {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncReadExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_bg = Arc::clone(&hits);
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let hits = Arc::clone(&hits_bg);
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    let mut req = Vec::new();
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        req.extend_from_slice(&buf[..n]);
                        if req.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    hits.fetch_add(1, Ordering::SeqCst);
                    let first_line = String::from_utf8_lossy(&req);
                    let path = first_line
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let (status, body) = handler(path);
                    let reason = match status {
                        200 => "OK",
                        404 => "Not Found",
                        500 => "Internal Server Error",
                        _ => "Error",
                    };
                    let head = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        MockServer {
            base_url: format!("http://{addr}"),
            hits,
            task,
        }
    }

    fn manifest_json(version: &str, platform_key: &str, payload_url: &str, sig: &str) -> Vec<u8> {
        serde_json::json!({
            "version": version,
            "platforms": {
                platform_key: {
                    "url": payload_url,
                    "signature": sig,
                }
            }
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn constants_and_update_config_endpoints() {
        assert_eq!(URL, GITHUB_URL);
        assert_eq!(DEFAULT_URLS, &[GITHUB_URL, WASABI_URL, S3_URL]);

        let def = UpdateConfig::default();
        assert!(def.enabled);
        assert_eq!(def.url, None);
        assert_eq!(def.endpoints(), DEFAULT_URLS);

        let empty_yaml: UpdateConfig = serde_yaml::from_str("{}").unwrap();
        assert!(empty_yaml.enabled);
        assert_eq!(empty_yaml.endpoints(), DEFAULT_URLS);

        let disabled_yaml: UpdateConfig = serde_yaml::from_str("enabled: false\n").unwrap();
        assert!(!disabled_yaml.enabled);
        assert_eq!(disabled_yaml.endpoints(), DEFAULT_URLS);

        let null_url_yaml: UpdateConfig = serde_yaml::from_str("url: ~\n").unwrap();
        assert_eq!(null_url_yaml.endpoints(), DEFAULT_URLS);

        let blank_url_yaml: UpdateConfig = serde_yaml::from_str("url: \"   \"\n").unwrap();
        assert_eq!(blank_url_yaml.endpoints(), DEFAULT_URLS);

        let custom_yaml: UpdateConfig =
            serde_yaml::from_str("url: https://scratch.example/latest.json\n").unwrap();
        assert_eq!(
            custom_yaml.endpoints(),
            vec!["https://scratch.example/latest.json"]
        );
    }

    #[tokio::test]
    async fn fallback_primary_succeeds_without_contacting_secondary() {
        let key = host_platform_key();
        let tgz = test_tgz();
        let payload_holder = Arc::new(std::sync::OnceLock::<String>::new());
        let payload_ref = Arc::clone(&payload_holder);
        let primary = spawn_mock_server(move |path| match path {
            "/latest.json" => (
                200,
                manifest_json("2.2.0", &key, payload_ref.get().unwrap(), TEST_SIG),
            ),
            "/payload.tar.gz" => (200, tgz.clone()),
            _ => (404, Vec::new()),
        })
        .await;
        payload_holder.set(primary.url("/payload.tar.gz")).unwrap();

        let secondary = spawn_mock_server(|_| (200, Vec::new())).await;

        let (dir, exe) = installed();
        let data_dir = tempfile::tempdir().unwrap();
        let endpoints = [primary.url("/latest.json"), secondary.url("/latest.json")];
        let endpoint_refs: Vec<&str> = endpoints.iter().map(String::as_str).collect();

        let outcome = check_and_apply_with(
            &endpoint_refs,
            data_dir.path(),
            &exe,
            &v("2.1.0"),
            None,
            TEST_PUBKEY,
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            Outcome::Applied {
                from: "2.1.0".into(),
                to: "2.2.0".into(),
            }
        );
        assert_eq!(std::fs::read(&exe).unwrap(), TEST_BINARY);
        assert_eq!(primary.hits(), 2);
        assert_eq!(secondary.hits(), 0, "fallback must not be contacted");
        assert!(names(&data_dir.path().join("updates")).is_empty());
        drop(dir);
    }

    #[tokio::test]
    async fn fallback_primary_http_404_and_500_uses_secondary() {
        for status in [404u16, 500u16] {
            let primary = spawn_mock_server(move |_| (status, b"error".to_vec())).await;

            let key = host_platform_key();
            let tgz = test_tgz();
            let payload_holder = Arc::new(std::sync::OnceLock::<String>::new());
            let payload_ref = Arc::clone(&payload_holder);
            let secondary = spawn_mock_server(move |path| match path {
                "/latest.json" => (
                    200,
                    manifest_json("2.2.0", &key, payload_ref.get().unwrap(), TEST_SIG),
                ),
                "/payload.tar.gz" => (200, tgz.clone()),
                _ => (404, Vec::new()),
            })
            .await;
            payload_holder
                .set(secondary.url("/payload.tar.gz"))
                .unwrap();

            let (dir, exe) = installed();
            let data_dir = tempfile::tempdir().unwrap();
            let endpoints = [primary.url("/latest.json"), secondary.url("/latest.json")];
            let endpoint_refs: Vec<&str> = endpoints.iter().map(String::as_str).collect();

            let outcome = check_and_apply_with(
                &endpoint_refs,
                data_dir.path(),
                &exe,
                &v("2.1.0"),
                None,
                TEST_PUBKEY,
            )
            .await
            .unwrap();

            assert_eq!(
                outcome,
                Outcome::Applied {
                    from: "2.1.0".into(),
                    to: "2.2.0".into(),
                },
                "status {status}"
            );
            assert_eq!(std::fs::read(&exe).unwrap(), TEST_BINARY);
            assert_eq!(primary.hits(), 1);
            assert_eq!(secondary.hits(), 2);
            drop(dir);
        }
    }

    #[tokio::test]
    async fn fallback_primary_payload_error_uses_secondary() {
        for payload_status in [500u16, 404u16] {
            let key = host_platform_key();
            let p_holder = Arc::new(std::sync::OnceLock::<String>::new());
            let p_ref = Arc::clone(&p_holder);
            let p_key = key.clone();
            let primary = spawn_mock_server(move |path| match path {
                "/latest.json" => (
                    200,
                    manifest_json("2.2.0", &p_key, p_ref.get().unwrap(), TEST_SIG),
                ),
                "/payload.tar.gz" => (payload_status, b"broken payload".to_vec()),
                _ => (404, Vec::new()),
            })
            .await;
            p_holder.set(primary.url("/payload.tar.gz")).unwrap();

            let tgz = test_tgz();
            let s_holder = Arc::new(std::sync::OnceLock::<String>::new());
            let s_ref = Arc::clone(&s_holder);
            let secondary = spawn_mock_server(move |path| match path {
                "/latest.json" => (
                    200,
                    manifest_json("2.2.0", &key, s_ref.get().unwrap(), TEST_SIG),
                ),
                "/payload.tar.gz" => (200, tgz.clone()),
                _ => (404, Vec::new()),
            })
            .await;
            s_holder.set(secondary.url("/payload.tar.gz")).unwrap();

            let (dir, exe) = installed();
            let data_dir = tempfile::tempdir().unwrap();
            let endpoints = [primary.url("/latest.json"), secondary.url("/latest.json")];
            let endpoint_refs: Vec<&str> = endpoints.iter().map(String::as_str).collect();

            let outcome = check_and_apply_with(
                &endpoint_refs,
                data_dir.path(),
                &exe,
                &v("2.1.0"),
                None,
                TEST_PUBKEY,
            )
            .await
            .unwrap();

            assert_eq!(
                outcome,
                Outcome::Applied {
                    from: "2.1.0".into(),
                    to: "2.2.0".into(),
                },
                "payload status {payload_status}"
            );
            assert_eq!(std::fs::read(&exe).unwrap(), TEST_BINARY);
            assert_eq!(primary.hits(), 2, "primary manifest + failed payload");
            assert_eq!(secondary.hits(), 2, "secondary manifest + payload");
            assert!(
                names(&data_dir.path().join("updates")).is_empty(),
                "partial temp files must be cleaned up"
            );
            drop(dir);
        }
    }

    #[tokio::test]
    async fn fallback_primary_up_to_date_does_not_contact_secondary() {
        for remote_ver in ["2.1.0", "2.0.9", "3.0.0"] {
            let key = host_platform_key();
            let primary = spawn_mock_server(move |_| {
                (
                    200,
                    manifest_json(remote_ver, &key, "http://127.0.0.1:1/unused", TEST_SIG),
                )
            })
            .await;
            let secondary = spawn_mock_server(|_| (200, Vec::new())).await;

            let (dir, exe) = installed();
            let data_dir = tempfile::tempdir().unwrap();
            let endpoints = [primary.url("/latest.json"), secondary.url("/latest.json")];
            let endpoint_refs: Vec<&str> = endpoints.iter().map(String::as_str).collect();

            let outcome = check_and_apply_with(
                &endpoint_refs,
                data_dir.path(),
                &exe,
                &v("2.1.0"),
                None,
                TEST_PUBKEY,
            )
            .await
            .unwrap();

            assert_eq!(outcome, Outcome::UpToDate, "remote {remote_ver}");
            assert_eq!(primary.hits(), 1);
            assert_eq!(secondary.hits(), 0, "fallback must not be contacted");
            drop(dir);
        }
    }

    #[tokio::test]
    async fn fallback_both_endpoints_fail_returns_error() {
        let primary = spawn_mock_server(|_| (404, b"not found".to_vec())).await;
        let secondary = spawn_mock_server(|_| (500, b"boom".to_vec())).await;

        let (dir, exe) = installed();
        let data_dir = tempfile::tempdir().unwrap();
        let endpoints = [primary.url("/latest.json"), secondary.url("/latest.json")];
        let endpoint_refs: Vec<&str> = endpoints.iter().map(String::as_str).collect();

        let err = check_and_apply_with(
            &endpoint_refs,
            data_dir.path(),
            &exe,
            &v("2.1.0"),
            None,
            TEST_PUBKEY,
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(&endpoints[0]), "missing primary url: {msg}");
        assert!(msg.contains(&endpoints[1]), "missing secondary url: {msg}");
        assert_eq!(primary.hits(), 1);
        assert_eq!(secondary.hits(), 1);
        drop(dir);
    }

    #[tokio::test]
    async fn explicit_custom_url_does_not_fall_back() {
        let custom = spawn_mock_server(|_| (500, b"custom fail".to_vec())).await;
        let cfg = UpdateConfig {
            enabled: true,
            url: Some(custom.url("/latest.json")),
            interval_secs: None,
        };
        let endpoints = cfg.endpoints();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0], custom.url("/latest.json"));

        let (dir, exe) = installed();
        let data_dir = tempfile::tempdir().unwrap();
        let err = check_and_apply_with(
            &endpoints,
            data_dir.path(),
            &exe,
            &v("2.1.0"),
            None,
            TEST_PUBKEY,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains(&custom.url("/latest.json")));
        assert_eq!(custom.hits(), 1);
        drop(dir);
    }

    #[tokio::test]
    async fn fallback_primary_missing_platform_uses_secondary() {
        let primary = spawn_mock_server(|_| {
            (
                200,
                manifest_json(
                    "2.2.0",
                    "cli-otheros-otherarch",
                    "http://127.0.0.1:1/unused",
                    TEST_SIG,
                ),
            )
        })
        .await;

        let key = host_platform_key();
        let tgz = test_tgz();
        let s_holder = Arc::new(std::sync::OnceLock::<String>::new());
        let s_ref = Arc::clone(&s_holder);
        let secondary = spawn_mock_server(move |path| match path {
            "/latest.json" => (
                200,
                manifest_json("2.2.0", &key, s_ref.get().unwrap(), TEST_SIG),
            ),
            "/payload.tar.gz" => (200, tgz.clone()),
            _ => (404, Vec::new()),
        })
        .await;
        s_holder.set(secondary.url("/payload.tar.gz")).unwrap();

        let (dir, exe) = installed();
        let data_dir = tempfile::tempdir().unwrap();
        let endpoints = [primary.url("/latest.json"), secondary.url("/latest.json")];
        let endpoint_refs: Vec<&str> = endpoints.iter().map(String::as_str).collect();

        let outcome = check_and_apply_with(
            &endpoint_refs,
            data_dir.path(),
            &exe,
            &v("2.1.0"),
            None,
            TEST_PUBKEY,
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            Outcome::Applied {
                from: "2.1.0".into(),
                to: "2.2.0".into(),
            }
        );
        assert_eq!(std::fs::read(&exe).unwrap(), TEST_BINARY);
        assert_eq!(primary.hits(), 1);
        assert_eq!(secondary.hits(), 2);
        drop(dir);
    }

    #[tokio::test]
    async fn bad_signature_is_rejected_and_leaves_exe_intact() {
        let key = host_platform_key();
        let mut tampered = test_tgz();
        tampered[0] ^= 0xff;
        let p_holder = Arc::new(std::sync::OnceLock::<String>::new());
        let p_ref = Arc::clone(&p_holder);
        let primary = spawn_mock_server(move |path| match path {
            "/latest.json" => (
                200,
                manifest_json("2.2.0", &key, p_ref.get().unwrap(), TEST_SIG),
            ),
            "/payload.tar.gz" => (200, tampered.clone()),
            _ => (404, Vec::new()),
        })
        .await;
        p_holder.set(primary.url("/payload.tar.gz")).unwrap();

        let (dir, exe) = installed();
        let data_dir = tempfile::tempdir().unwrap();
        let endpoints = [primary.url("/latest.json")];
        let endpoint_refs: Vec<&str> = endpoints.iter().map(String::as_str).collect();

        let err = check_and_apply_with(
            &endpoint_refs,
            data_dir.path(),
            &exe,
            &v("2.1.0"),
            None,
            TEST_PUBKEY,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("signature does not verify"));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert!(names(&data_dir.path().join("updates")).is_empty());
        drop(dir);
    }

    #[test]
    fn update_config_check_interval_defaults_custom_and_clamps() {
        assert_eq!(EVERY, Duration::from_secs(30 * 60));
        assert_eq!(JITTER_SECS, 120);

        let def = UpdateConfig::default();
        assert_eq!(def.interval_secs, None);
        assert_eq!(def.check_interval(), Duration::from_secs(30 * 60));

        let custom = UpdateConfig {
            interval_secs: Some(600),
            ..UpdateConfig::default()
        };
        assert_eq!(custom.check_interval(), Duration::from_secs(600));

        let exact_min = UpdateConfig {
            interval_secs: Some(60),
            ..UpdateConfig::default()
        };
        assert_eq!(exact_min.check_interval(), Duration::from_secs(60));

        for too_low in [0, 1, 59] {
            let clamped = UpdateConfig {
                interval_secs: Some(too_low),
                ..UpdateConfig::default()
            };
            assert_eq!(
                clamped.check_interval(),
                Duration::from_secs(60),
                "interval_secs={too_low} must clamp to 60s"
            );
        }

        let from_empty: UpdateConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(from_empty.check_interval(), Duration::from_secs(30 * 60));

        let from_null: UpdateConfig = serde_yaml::from_str("interval_secs: ~\n").unwrap();
        assert_eq!(from_null.check_interval(), Duration::from_secs(30 * 60));

        let from_custom: UpdateConfig = serde_yaml::from_str("interval_secs: 900\n").unwrap();
        assert_eq!(from_custom.check_interval(), Duration::from_secs(900));

        let from_clamped: UpdateConfig = serde_yaml::from_str("interval_secs: 15\n").unwrap();
        assert_eq!(from_clamped.check_interval(), Duration::from_secs(60));
    }

    async fn spawn_test_actor(
        managed: bool,
        attached: Arc<std::sync::atomic::AtomicUsize>,
    ) -> (Runtime, ActorRef<UpdateActor>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let actor = UpdateActor::spawn_with_mailbox(
            UpdateActor {
                rt: rt.clone(),
                ready: None,
                attached: Some(Arc::new(move || {
                    attached.load(std::sync::atomic::Ordering::SeqCst)
                })),
                managed: Arc::new(move || managed),
                last_turn: Instant::now(),
                active_turns: HashSet::new(),
                waiting_logged: None,
            },
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<UpdateActor, agent::AgentEvent>(&actor).await;
        (rt, actor, dir)
    }

    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    fn turn_started_event(session: runtime::SessionId, turn: runtime::TurnId) -> agent::AgentEvent {
        agent::AgentEvent::TurnStarted {
            session,
            turn,
            text: "hi".into(),
            model: "test".into(),
            turn_seq: 1,
            channel: None,
        }
    }

    fn turn_finished_event(
        session: runtime::SessionId,
        turn: runtime::TurnId,
        stop_reason: &str,
    ) -> agent::AgentEvent {
        agent::AgentEvent::TurnFinished {
            session,
            turn,
            stop_reason: stop_reason.into(),
            usage: agent::Usage::default(),
            context: 0,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn managed_daemon_restarts_after_idle_without_waiting_for_every() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let attached = Arc::new(AtomicUsize::new(0));
        let (rt, actor, _dir) = spawn_test_actor(true, Arc::clone(&attached)).await;
        let checks = Arc::new(AtomicUsize::new(0));
        let checks_bg = Arc::clone(&checks);

        let task = tokio::spawn(run_tick_loop(
            rt.clone(),
            actor.clone(),
            EVERY,
            0,
            move |_applied| {
                checks_bg.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(Outcome::Applied {
                        from: "2.31.2".into(),
                        to: "2.32.0".into(),
                    })
                }
            },
        ));
        settle().await;

        // First check runs at FIRST_CHECK (60 s); daemon is only 60 s old (< 10 min idle).
        tokio::time::advance(FIRST_CHECK).await;
        settle().await;
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        assert!(!task.is_finished(), "must wait for IDLE_BEFORE_EXIT");

        // Advance one IDLE_POLL at a time up to just before IDLE_BEFORE_EXIT (9m30s total):
        // still waiting, and no extra manifest fetch occurs.
        let polls_before_ready = (IDLE_BEFORE_EXIT - FIRST_CHECK).as_secs() / IDLE_POLL.as_secs() - 1;
        for _ in 0..polls_before_ready {
            tokio::time::advance(IDLE_POLL).await;
            settle().await;
            assert!(!task.is_finished(), "must not restart before IDLE_BEFORE_EXIT");
        }
        assert_eq!(
            checks.load(Ordering::SeqCst),
            1,
            "idle polls must not re-fetch the network manifest"
        );

        // One more IDLE_POLL crosses IDLE_BEFORE_EXIT (10 min << EVERY 30 min): restarts now.
        tokio::time::advance(IDLE_POLL).await;
        settle().await;
        assert!(task.is_finished(), "must restart once IDLE_BEFORE_EXIT elapses");
        assert!(task.await.unwrap());
        assert_eq!(checks.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn active_turn_blocks_restart_until_finished_and_idle() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let attached = Arc::new(AtomicUsize::new(0));
        let (rt, actor, _dir) = spawn_test_actor(true, Arc::clone(&attached)).await;
        let checks = Arc::new(AtomicUsize::new(0));
        let checks_bg = Arc::clone(&checks);

        let s1 = runtime::SessionId::new();
        let t1 = runtime::TurnId::new();
        let s2 = runtime::SessionId::new();
        let t2 = runtime::TurnId::new();
        let s3 = runtime::SessionId::new();
        let t3 = runtime::TurnId::new();
        let s4 = runtime::SessionId::new();
        let t4 = runtime::TurnId::new();

        // Start four turns via both TurnStarted and TurnResumed.
        rt.publish(turn_started_event(s1, t1)).await;
        rt.publish(agent::AgentEvent::TurnResumed {
            session: s2,
            turn: t2,
            kind: agent::durability::ResumeKind::Suspended,
            interrupted_at: chrono::Utc::now(),
            resumed_at: chrono::Utc::now(),
            partial_dropped: false,
            cause: agent::durability::ResumeCause::Update,
            from: None,
        })
        .await;
        rt.publish(turn_started_event(s3, t3)).await;
        rt.publish(turn_started_event(s4, t4)).await;
        settle().await;

        let task = tokio::spawn(run_tick_loop(
            rt.clone(),
            actor.clone(),
            EVERY,
            0,
            move |_applied| {
                checks_bg.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(Outcome::Applied {
                        from: "2.31.2".into(),
                        to: "2.32.0".into(),
                    })
                }
            },
        ));
        settle().await;

        // First check applies the update at t=60 s.
        tokio::time::advance(FIRST_CHECK).await;
        settle().await;
        assert_eq!(checks.load(Ordering::SeqCst), 1);

        // Advance well past IDLE_BEFORE_EXIT (15 min) while turns are still in flight.
        tokio::time::advance(IDLE_BEFORE_EXIT + Duration::from_secs(5 * 60)).await;
        settle().await;
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        assert!(
            !task.is_finished(),
            "active turns must block restart even past IDLE_BEFORE_EXIT"
        );

        // End three of the four turns (normal, cancelled, error) — one still active.
        rt.publish(turn_finished_event(s1, t1, "end_turn")).await;
        rt.publish(turn_finished_event(s2, t2, "cancelled")).await;
        rt.publish(turn_finished_event(s3, t3, "error")).await;
        tokio::time::advance(IDLE_POLL).await;
        settle().await;
        assert!(!task.is_finished(), "remaining active turn must still block");

        // End the final turn (suspended): active_turns becomes empty and idle clock resets.
        rt.publish(agent::AgentEvent::TurnSuspended {
            session: s4,
            turn: t4,
            reason: "transfer".into(),
        })
        .await;
        settle().await;

        // Step IDLE_POLL up to just before IDLE_BEFORE_EXIT after the turn ended: still waiting.
        let polls_before_ready = IDLE_BEFORE_EXIT.as_secs() / IDLE_POLL.as_secs() - 1;
        for _ in 0..polls_before_ready {
            tokio::time::advance(IDLE_POLL).await;
            settle().await;
            assert!(
                !task.is_finished(),
                "ending a turn must reset the idle window"
            );
        }

        // Once IDLE_BEFORE_EXIT elapses after the turn ended: restarts cleanly.
        tokio::time::advance(IDLE_POLL).await;
        settle().await;
        assert!(task.is_finished(), "must restart once idle after turn ends");
        assert!(task.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn attached_client_blocks_restart_until_detached_and_idle() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let attached = Arc::new(AtomicUsize::new(1));
        let (rt, actor, _dir) = spawn_test_actor(true, Arc::clone(&attached)).await;
        let checks = Arc::new(AtomicUsize::new(0));
        let checks_bg = Arc::clone(&checks);

        let task = tokio::spawn(run_tick_loop(
            rt.clone(),
            actor.clone(),
            EVERY,
            0,
            move |_applied| {
                checks_bg.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(Outcome::Applied {
                        from: "2.31.2".into(),
                        to: "2.32.0".into(),
                    })
                }
            },
        ));
        settle().await;

        // First check applies the update at t=60 s.
        tokio::time::advance(FIRST_CHECK).await;
        settle().await;
        assert_eq!(checks.load(Ordering::SeqCst), 1);

        // Advance past IDLE_BEFORE_EXIT while a client is attached.
        tokio::time::advance(IDLE_BEFORE_EXIT + Duration::from_secs(60)).await;
        settle().await;
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        assert!(
            !task.is_finished(),
            "attached client must block restart even when idle"
        );

        // Client disconnects: next IDLE_POLL restarts without waiting for EVERY or re-fetching.
        attached.store(0, Ordering::SeqCst);
        tokio::time::advance(IDLE_POLL).await;
        settle().await;
        assert!(task.is_finished(), "must restart once client detaches");
        assert!(task.await.unwrap());
        assert_eq!(checks.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unmanaged_daemon_does_not_restart() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let attached = Arc::new(AtomicUsize::new(0));
        let (rt, actor, _dir) = spawn_test_actor(false, Arc::clone(&attached)).await;
        let checks = Arc::new(AtomicUsize::new(0));
        let checks_bg = Arc::clone(&checks);

        let task = tokio::spawn(run_tick_loop(
            rt.clone(),
            actor.clone(),
            EVERY,
            0,
            move |_applied| {
                checks_bg.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(Outcome::Applied {
                        from: "2.31.2".into(),
                        to: "2.32.0".into(),
                    })
                }
            },
        ));
        settle().await;

        tokio::time::advance(FIRST_CHECK).await;
        settle().await;
        tokio::time::advance(IDLE_BEFORE_EXIT + Duration::from_secs(60)).await;
        settle().await;
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        assert!(!task.is_finished(), "unmanaged daemon must never self-restart");
        assert_eq!(
            actor.ask(Status).await.unwrap(),
            Some(UpdateReady {
                version: "2.32.0".into(),
                applied: true,
            })
        );

        rt.shutdown.cancel();
        assert!(!task.await.unwrap());
    }
}
