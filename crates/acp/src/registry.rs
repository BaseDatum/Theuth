//! The public ACP agent registry (design doc 06 §1, §3.1): fetched into
//! `<data_dir>/acp/registry.json`, refetched when older than a day, and turned
//! into [`Entry`]s. `binary` distributions download into `<data_dir>/acp/bin/<id>/`
//! and shell out to `tar` / `unzip` (06 §8.14).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents::{DIR, Entry, Reports};

/// Where the registry lives.
pub const URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
/// Cache file name inside [`DIR`].
const CACHE_FILE: &str = "registry.json";
/// Cache lifetime.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// One registry agent.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RegistryAgent {
    /// Registry id (`codex-acp`); also the `bin/<id>` directory for binary installs.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Version string.
    #[serde(default)]
    pub version: String,
    /// One-line description.
    #[serde(default)]
    pub description: String,
    /// `{binary: {<platform>: {archive, sha256, cmd, args, env}}, npx: {package, args}, uvx: {package, args}}`.
    pub distribution: Value,
}

/// Top-level registry file.
#[derive(Deserialize)]
struct Registry {
    agents: Vec<RegistryAgent>,
}

/// `npx` / `uvx` distribution.
#[derive(Deserialize)]
struct Package {
    package: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// One platform of a `binary` distribution.
#[derive(Deserialize)]
struct Binary {
    archive: String,
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
struct Distribution {
    npx: Option<Package>,
    uvx: Option<Package>,
    #[serde(default)]
    binary: BTreeMap<String, Binary>,
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR).join(CACHE_FILE)
}

fn parse(text: &str) -> anyhow::Result<Vec<RegistryAgent>> {
    Ok(serde_json::from_str::<Registry>(text)?.agents)
}

/// The registry: the cache when younger than a day, else a fresh fetch (cached
/// on success); offline falls back to any cache, or Err when there is none.
pub async fn fetch(data_dir: &Path) -> anyhow::Result<Vec<RegistryAgent>> {
    let path = cache_path(data_dir);
    let cached = std::fs::read_to_string(&path).ok();
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < MAX_AGE);
    if fresh
        && let Some(text) = &cached
        && let Ok(agents) = parse(text)
    {
        return Ok(agents);
    }
    let fetched = async {
        let text = reqwest::get(URL).await?.error_for_status()?.text().await?;
        let agents = parse(&text)?;
        anyhow::Ok((text, agents))
    }
    .await;
    match fetched {
        Ok((text, agents)) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = platform::files::write_atomic(&path, text.as_bytes()) {
                tracing::warn!("acp registry cache: {e}");
            }
            Ok(agents)
        }
        Err(e) => match cached.as_deref().map(parse) {
            Some(Ok(agents)) => {
                tracing::warn!("acp registry fetch failed, using cache: {e:#}");
                Ok(agents)
            }
            _ => Err(e).context("fetch ACP registry"),
        },
    }
}

/// `darwin-aarch64` | `darwin-x86_64` | `linux-aarch64` | `linux-x86_64`.
fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

fn description(agent: &RegistryAgent) -> Option<String> {
    let d = agent.description.trim();
    (!d.is_empty()).then(|| d.to_owned())
}

/// The entry for a `npx` / `uvx` distribution (06 §3.1); `Ok(None)` when the
/// distribution has neither (binary only).
pub fn entry_from_distribution(
    dist: &Value,
    description: Option<String>,
) -> anyhow::Result<Option<Entry>> {
    let dist: Distribution = serde_json::from_value(dist.clone()).context("distribution")?;
    let (command, package) = match (&dist.npx, &dist.uvx) {
        (Some(p), _) => ("npx", p),
        (None, Some(p)) => ("uvx", p),
        (None, None) => return Ok(None),
    };
    let mut args = Vec::with_capacity(package.args.len() + 2);
    if command == "npx" {
        args.push("-y".to_owned());
    }
    args.push(package.package.clone());
    args.extend(package.args.iter().cloned());
    Ok(Some(Entry {
        command: command.to_owned(),
        args,
        env: package.env.clone(),
        description,
        model: None,
        skills: None,
        skills_dir: None,
        mcp: None,
        reports: None,
        steering: Default::default(),
    }))
}

/// Build an [`Entry`] from a registry agent: `npx`/`uvx` map to a command line;
/// `binary` uses the same-named executable when it is already on `PATH`, else
/// downloads this platform's archive into `<data_dir>/acp/bin/<id>/`, verifies
/// `sha256`, extracts it, and points `command` at the extracted `cmd`.
pub async fn install(data_dir: &Path, agent: &RegistryAgent) -> anyhow::Result<Entry> {
    let description = description(agent);
    if let Some(mut entry) = entry_from_distribution(&agent.distribution, description.clone())? {
        entry.reports = Some(Reports::for_registry(&agent.id));
        return Ok(entry);
    }
    let dist: Distribution =
        serde_json::from_value(agent.distribution.clone()).context("distribution")?;
    let platform = platform();
    let Some(bin) = dist.binary.get(&platform) else {
        let have: Vec<&str> = dist.binary.keys().map(String::as_str).collect();
        bail!(
            "{}: no distribution for {platform} (binary: {})",
            agent.id,
            if have.is_empty() {
                "none".to_owned()
            } else {
                have.join(", ")
            }
        );
    };
    if let Some(command) = on_path(bin.cmd.trim_start_matches("./")) {
        return Ok(Entry {
            command: command.to_string_lossy().into_owned(),
            args: bin.args.clone(),
            env: bin.env.clone(),
            description,
            model: None,
            skills: None,
            skills_dir: None,
            mcp: None,
            reports: Some(Reports::for_registry(&agent.id)),
            steering: Default::default(),
        });
    }

    let dir = bin_dir(data_dir, &agent.id);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let bytes = reqwest::get(&bin.archive)
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("download {}", bin.archive))?
        .bytes()
        .await
        .with_context(|| format!("download {}", bin.archive))?;
    if let Some(want) = &bin.sha256 {
        let got = platform::sha256::hex(&bytes);
        if !got.eq_ignore_ascii_case(want.trim()) {
            bail!(
                "{}: sha256 mismatch (expected {want}, got {got})",
                bin.archive
            );
        }
    }
    let file_name = bin
        .archive
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("archive");
    let archive = dir.join(file_name);
    tokio::fs::write(&archive, &bytes)
        .await
        .with_context(|| format!("write {}", archive.display()))?;

    let mut cmd = if file_name.ends_with(".zip") {
        let mut c = tokio::process::Command::new("unzip");
        c.arg("-o").arg(&archive).arg("-d").arg(&dir);
        c
    } else {
        let mut c = tokio::process::Command::new("tar");
        c.arg("xf").arg(&archive).arg("-C").arg(&dir);
        c
    };
    platform::process::hide_console(cmd.as_std_mut());
    let out = cmd
        .output()
        .await
        .with_context(|| format!("run {:?}", cmd.as_std().get_program()))?;
    if !out.status.success() {
        bail!(
            "extract {}: {}\n{}",
            archive.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let _ = std::fs::remove_file(&archive);

    let command = dir.join(bin.cmd.trim_start_matches("./"));
    if !command.exists() {
        bail!("{}: extracted archive has no {}", agent.id, bin.cmd);
    }
    Ok(Entry {
        command: command.to_string_lossy().into_owned(),
        args: bin.args.clone(),
        env: bin.env.clone(),
        description,
        model: None,
        skills: None,
        skills_dir: None,
        mcp: None,
        reports: Some(Reports::for_registry(&agent.id)),
        steering: Default::default(),
    })
}

/// The user's own install of `name`, if `PATH` has one.
fn on_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return None;
    }
    platform::shell::which(name)
}

fn bin_dir(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join(DIR).join("bin").join(id)
}

/// Delete `<data_dir>/acp/bin/<id>` if present.
pub fn remove_bin(data_dir: &Path, id: &str) -> std::io::Result<()> {
    match std::fs::remove_dir_all(bin_dir(data_dir, id)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"version":"1.0.0","agents":[
      {"id":"codex-acp","name":"Codex","version":"1.11.0","description":"OpenAI Codex",
       "distribution":{"npx":{"package":"@agentclientprotocol/codex-acp@1.11.0"}}},
      {"id":"opencode","name":"OpenCode","version":"1.18.20",
       "distribution":{"binary":{"darwin-aarch64":{"archive":"https://x/opencode-darwin-arm64.zip","cmd":"./opencode","args":["acp"],"sha256":"a5e4"}}}},
      {"id":"py","name":"Py","version":"0.1","distribution":{"uvx":{"package":"py-acp","args":["--acp"]}}}
    ],"extensions":[]}"#;

    #[test]
    fn registry_sample_parses() {
        let agents = parse(SAMPLE).unwrap();
        assert_eq!(agents.len(), 3);
        assert_eq!(agents[0].id, "codex-acp");
        assert_eq!(agents[0].description, "OpenAI Codex");
        assert_eq!(agents[1].description, "");
        assert_eq!(
            agents[1].distribution["binary"]["darwin-aarch64"]["cmd"],
            "./opencode"
        );
    }

    #[test]
    fn npx_and_uvx_entries() {
        let agents = parse(SAMPLE).unwrap();
        let codex = entry_from_distribution(&agents[0].distribution, description(&agents[0]))
            .unwrap()
            .unwrap();
        assert_eq!(codex.command, "npx");
        assert_eq!(codex.args, ["-y", "@agentclientprotocol/codex-acp@1.11.0"]);
        assert_eq!(codex.description.as_deref(), Some("OpenAI Codex"));

        let py = entry_from_distribution(&agents[2].distribution, None)
            .unwrap()
            .unwrap();
        assert_eq!(py.command, "uvx");
        assert_eq!(py.args, ["py-acp", "--acp"]);
        assert_eq!(py.description, None);

        assert!(
            entry_from_distribution(&agents[1].distribution, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn remove_missing_bin_is_ok() {
        let dir = std::env::temp_dir().join(format!("theuth-acp-reg-{}", std::process::id()));
        remove_bin(&dir, "nope").unwrap();
        std::fs::create_dir_all(bin_dir(&dir, "x")).unwrap();
        remove_bin(&dir, "x").unwrap();
        assert!(!bin_dir(&dir, "x").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
