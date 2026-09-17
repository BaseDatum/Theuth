//! Update plugin: check the release manifest, verify, replace `wth` on disk;
//! never ask (design doc 39).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`UPDATE_ACTOR`] actor ([`UpdateActor`]): [`Status`], [`SetAttached`]
//! - events: [`UpdateReady`]
//! - [`run_once`] for `wth --update`

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt as _;
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

/// Where `latest.json` lives (39 §3.1). `update.url` overrides it.
pub const URL: &str = "https://s3.rwx.dev/wth/releases/latest.json";

/// The minisign public key every payload is verified against (39 §5.1), in
/// Tauri's encoding (the `.pub` file, base64). Shared with the desktop's
/// `tauri.conf.json` by value; the release script checks the two match.
pub const PUBKEY: &str = include_str!("../pubkey.txt");

/// Delay before the first check after start (39 §9).
const FIRST_CHECK: Duration = Duration::from_secs(60);
/// Interval between checks (39 §9).
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Upper bound of the per-tick jitter added to [`EVERY`] (39 §9).
const JITTER_SECS: u64 = 5 * 60;
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
    /// Manifest URL; the default is [`URL`]. A scratch bucket for checks.
    pub url: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            url: URL.to_owned(),
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
#[serde(tag = "type")]
pub struct UpdateReady {
    /// The version that is ready (`0.2.0`).
    pub version: String,
    /// `true`: the next start of `wth` runs it. `false`: `wth --update`
    /// (with `sudo`) installs it.
    pub applied: bool,
}

/// `ask` → `Option<UpdateReady>`: the latest event, if any (39 §4.3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status;

/// `tell` from the rpc plugin at start: how many frontends are attached, so
/// a service-managed daemon only restarts itself when nobody is listening
/// (39 §3.4). A downward edge like `link`'s `SetDispatch`.
pub struct SetAttached(pub Arc<dyn Fn() -> usize + Send + Sync>);

/// `ask` (internal, from the tick task) → `true` when the process should exit
/// for restart (39 §3.4). Carries what this tick found; the actor publishes
/// it when it is news.
struct Landed(Option<UpdateReady>);

/// The [`UPDATE_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct UpdateActor {
    rt: Runtime,
    ready: Option<UpdateReady>,
    attached: Option<Arc<dyn Fn() -> usize + Send + Sync>>,
    /// When the last turn started; `now` at spawn so a fresh daemon never
    /// exits inside its first [`IDLE_BEFORE_EXIT`].
    last_turn: Instant,
}

impl Message<Status> for UpdateActor {
    type Reply = Option<UpdateReady>;
    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ready.clone()
    }
}

impl Message<SetAttached> for UpdateActor {
    type Reply = ();
    async fn handle(&mut self, m: SetAttached, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.attached = Some(m.0);
    }
}

impl Message<agent::AgentEvent> for UpdateActor {
    type Reply = ();
    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        if matches!(ev, agent::AgentEvent::TurnStarted { .. }) {
            self.last_turn = Instant::now();
        }
    }
}

impl Message<Landed> for UpdateActor {
    type Reply = bool;
    async fn handle(&mut self, m: Landed, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(ready) = m.0
            && self.ready.as_ref() != Some(&ready)
        {
            self.rt.publish(ready.clone()).await;
            self.ready = Some(ready);
        }
        // 39 §3.4: (a) under a service manager, (b) nobody attached, (c) idle.
        let Some(ready) = self.ready.as_ref().filter(|r| r.applied) else {
            return false;
        };
        let managed =
            std::os::unix::process::parent_id() == 1 || std::env::var_os("INVOCATION_ID").is_some();
        let alone = self.attached.as_ref().is_some_and(|f| f() == 0);
        if managed && alone && self.last_turn.elapsed() >= IDLE_BEFORE_EXIT {
            tracing::info!("update: {} in place, exiting for restart", ready.version);
            return true;
        }
        false
    }
}

/// What one foreground check did (`wth --update`).
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
    Version::parse(env!("CARGO_PKG_VERSION")).context("CARGO_PKG_VERSION is not semver")
}

/// Tauri's encoding: base64 of the minisign text file (`.pub` / `.sig`).
fn tauri_b64(s: &str) -> anyhow::Result<String> {
    let bin = base64::prelude::BASE64_STANDARD.decode(s.trim())?;
    Ok(String::from_utf8(bin)?)
}

fn pubkey() -> anyhow::Result<PublicKey> {
    let text = tauri_b64(PUBKEY).map_err(|_| anyhow::anyhow!("update: pubkey not configured"))?;
    PublicKey::decode(&text).map_err(|_| anyhow::anyhow!("update: pubkey not configured"))
}

/// One check-and-apply in the foreground (39 §3.2 `wth --update`).
/// `data_dir` is the harness data dir; the tarball is spooled under
/// `<data_dir>/updates/`.
pub async fn run_once(cfg: &UpdateConfig, data_dir: &Path) -> anyhow::Result<Outcome> {
    check_and_apply(cfg, data_dir, &current_version()?, None).await
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
    let homebrew = exe.to_string_lossy().contains("/Cellar/wth/");

    // Step 1: the manifest.
    let client = reqwest::Client::builder()
        .connect_timeout(HTTP_TIMEOUT)
        .read_timeout(HTTP_TIMEOUT)
        .user_agent(format!("wth/{}", env!("CARGO_PKG_VERSION")))
        .build()?;
    let manifest: Manifest = client
        .get(&cfg.url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .with_context(|| format!("GET {}", cfg.url))?;
    let remote = Version::parse(&manifest.version)
        .with_context(|| format!("manifest version {:?}", manifest.version))?;
    if !newer(current, &remote) {
        return Ok(Outcome::UpToDate);
    }
    let version = manifest.version;
    let key = format!("cli-{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let Some(platform) = manifest.platforms.get(&key) else {
        tracing::debug!("update: {version} has no `{key}` entry");
        return Ok(Outcome::UpToDate);
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
        return Ok(done);
    }

    // Step 4: can we replace it?
    let dir = exe.parent().context("current_exe has no parent")?;
    if !writable(dir) {
        tracing::info!(
            "update: {version} is available but {} is not writable; `sudo wth --update` installs it",
            exe.display()
        );
        return Ok(Outcome::Available { version, path: exe });
    }

    // Steps 2–3: download + verify, extract, rename.
    let updates = data_dir.join("updates");
    tokio::fs::create_dir_all(&updates)
        .await
        .with_context(|| updates.display().to_string())?;
    let tarball = updates.join(format!("wth_{version}.tar.gz.part"));
    let result = match download(&client, &platform.url, &platform.signature, &tarball).await {
        Ok(()) => install(&tarball, &exe),
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&tarball);
    result?;
    Ok(done)
}

/// Step H: `brew upgrade --formula wth`, then confirm the Cellar holds `version`.
async fn brew_upgrade(version: &str) -> anyhow::Result<()> {
    let out = tokio::time::timeout(
        BREW_TIMEOUT,
        tokio::process::Command::new("brew")
            .args(["upgrade", "--formula", "wth"])
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
        .args(["list", "--versions", "wth"])
        .output()
        .await
        .context("brew list")?;
    let listed = String::from_utf8_lossy(&out.stdout);
    if !listed.split_whitespace().any(|v| v == version) {
        anyhow::bail!(
            "brew upgrade ran but `brew list --versions wth` says `{}`",
            listed.trim()
        );
    }
    Ok(())
}

/// Step 4 probe: can this user create a file in `dir`?
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".wth-update-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Step 2: stream `url` to `dest`, hashing as it lands; verify against
/// `signature` (Tauri-encoded `.sig`) before returning. `dest` is left in
/// place on error for the caller to remove.
async fn download(
    client: &reqwest::Client,
    url: &str,
    signature: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    let pk = pubkey()?;
    let sig = Signature::decode(&tauri_b64(signature).context("signature encoding")?)
        .context("signature")?;
    let mut verifier = pk.verify_stream(&sig).context("signature")?;
    let mut resp = client
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("GET {url}"))?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| dest.display().to_string())?;
    while let Some(chunk) = resp.chunk().await? {
        verifier.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    verifier.finalize().context("signature does not verify")?;
    Ok(())
}

/// Step 3: the entry `wth` from `tarball` → `<exe>.new`, `chmod 755`, rename
/// over `exe`. Same directory, so the rename is atomic and the running image
/// is untouched (39 §2.2 `ETXTBSY`).
fn install(tarball: &Path, exe: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(tarball).with_context(|| tarball.display().to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let new = exe.with_extension("new");
    let mut found = false;
    for entry in archive.entries().context("tarball")? {
        let mut entry = entry.context("tarball entry")?;
        if entry.path()?.file_name().is_some_and(|n| n == "wth")
            && entry.header().entry_type().is_file()
        {
            entry
                .unpack(&new)
                .with_context(|| new.display().to_string())?;
            found = true;
            break;
        }
    }
    if !found {
        anyhow::bail!("tarball has no `wth` entry");
    }
    let result = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
        .and_then(|()| std::fs::rename(&new, exe));
    if result.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    result.with_context(|| format!("replace {}", exe.display()))
}

/// The plugin (39 §3.2): registers [`UpdateActor`] as [`UPDATE_ACTOR`]; runs
/// the tick loop only when this process owns its binary and is a release build.
pub struct UpdatePlugin;

#[async_trait::async_trait]
impl runtime::Plugin for UpdatePlugin {
    fn name(&self) -> &'static str {
        "update"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: UpdateConfig = rt.section("update").unwrap_or_else(|e| {
            tracing::debug!("no [update] config section, using defaults: {e}");
            UpdateConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("update disabled by config");
            return Ok(());
        }
        let actor = UpdateActor::spawn_with_mailbox(
            UpdateActor {
                rt: rt.clone(),
                ready: None,
                attached: None,
                last_turn: Instant::now(),
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

/// The tick loop (39 §3.2): network in the task, state in the actor.
fn spawn_ticks(rt: Runtime, me: ActorRef<UpdateActor>, cfg: UpdateConfig, current: Version) {
    tokio::spawn(async move {
        let mut wait = FIRST_CHECK;
        loop {
            tokio::select! {
                () = rt.shutdown.cancelled() => break,
                () = tokio::time::sleep(wait) => {}
            }
            let applied = me
                .ask(Status)
                .await
                .ok()
                .flatten()
                .filter(|r| r.applied)
                .map(|r| r.version);
            let found =
                match check_and_apply(&cfg, &rt.data_dir, &current, applied.as_deref()).await {
                    Ok(outcome) => outcome.ready(),
                    Err(e) => {
                        tracing::debug!("update: {e:#}");
                        None
                    }
                };
            match me.ask(Landed(found)).await {
                Ok(true) => std::process::exit(EXIT_RESTART),
                Ok(false) => {}
                Err(_) => break,
            }
            wait = EVERY + Duration::from_secs(rand::random_range(0..JITTER_SECS));
        }
    });
}

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

    #[test]
    fn install_replaces_exe() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("wth");
        std::fs::write(&exe, b"old").unwrap();

        let payload = b"#!/bin/sh\necho new\n";
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (name, data) in [("._wth", &b"applesauce"[..]), ("wth", &payload[..])] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, data).unwrap();
        }
        let mut gz = tar.into_inner().unwrap();
        gz.flush().unwrap();
        let tarball = dir.path().join("wth_0.2.0.tar.gz.part");
        std::fs::write(&tarball, gz.finish().unwrap()).unwrap();

        install(&tarball, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), payload);
        assert_eq!(
            std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!exe.with_extension("new").exists());
    }
}
