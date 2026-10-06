//! Update plugin: check the release manifest, verify, replace `theuth` on disk;
//! never ask (design doc 39).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`UPDATE_ACTOR`] actor ([`UpdateActor`]): [`Status`], [`SetAttached`]
//! - events: [`UpdateReady`]
//! - [`run_once`] for `t --update`

use std::collections::HashMap;
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
pub const URL: &str = "https://s3.rwx.dev/theuth/releases/latest.json";

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
    /// `true`: the next start of `theuth` runs it. `false`: `t --update`
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
        let managed = platform::service::managed();
        let alone = self.attached.as_ref().is_some_and(|f| f() == 0);
        if managed && alone && self.last_turn.elapsed() >= IDLE_BEFORE_EXIT {
            tracing::info!("update: {} in place, exiting for restart", ready.version);
            return true;
        }
        false
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

/// One check-and-apply in the foreground (39 §3.2 `t --update`).
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
    let homebrew = exe.to_string_lossy().contains("/Cellar/theuth/");

    // Step 1: the manifest.
    let client = reqwest::Client::builder()
        .connect_timeout(HTTP_TIMEOUT)
        .read_timeout(HTTP_TIMEOUT)
        .user_agent(format!("theuth/{}", env!("CARGO_PKG_VERSION")))
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
            "update: {version} is available but {} is not writable; `sudo t --update` installs it",
            exe.display()
        );
        return Ok(Outcome::Available { version, path: exe });
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
    let result = match download(&client, &platform.url, &platform.signature, &tarball).await {
        Ok(()) => install(&tarball, &exe).await,
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&tarball);
    result?;
    Ok(done)
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

/// Step 3: the payload holds `theuth`, one `theuth-app-parser-<hash>` and at most one
/// `theuth-documents-<version>` (regular files, nothing else). All land beside `exe` as `<name>.new.<pid>-<nonce>`,
/// `chmod 755`; the new `theuth` is asked which helper hash it was compiled against
/// and must name the one shipped beside it; then swap: helper first, then
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
        Ok(hash) => probe(&staged[staged.len() - 1].0, &hash)
            .await
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
/// clean up. Returns the app parser helper's hash.
fn unpack(
    tarball: &Path,
    exe: &Path,
    dir: &Path,
    tag: &str,
    staged: &mut Vec<(PathBuf, PathBuf)>,
) -> anyhow::Result<String> {
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
    let Some(hash) = helper else {
        anyhow::bail!("tarball has no `{HELPER_PREFIX}<hash>` entry");
    };
    if !theuth {
        anyhow::bail!("tarball has no `t` entry");
    }
    // Commit order: helper first, `t` last.
    staged.sort_by_key(|(_, dest)| dest == exe);
    Ok(hash)
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
                Ok(true) => {
                    // 109 §9.3: a drain precedes the swap; the token then
                    // lets the jobs actor kill its groups as on any exit.
                    agent::supervisor::drain_engine(&rt, "update").await;
                    rt.shutdown.cancel();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    std::process::exit(EXIT_RESTART)
                }
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
            ("no helper", vec![file("t", b"new")]),
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
}
