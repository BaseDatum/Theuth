//! Named OpenAI-compatible gateways (`<data_dir>/providers.yaml`) plus their
//! keys in the secrets vault (37 §3.6), under the gateway's name. User-facing
//! ids are `name/model`; genai sees the OpenAI adapter with a rewritten endpoint.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;

use genai::adapter::AdapterKind;
use kameo::actor::ActorRef;
use models::{Limit, Model, Provider};
use serde::{Deserialize, Serialize};

/// One named OpenAI-compatible gateway.
#[derive(Clone, Debug)]
pub(crate) struct Gateway {
    /// Base URL, trailing slash (`https://api.example/v1/`).
    pub base_url: String,
    /// API key from the secrets vault.
    pub key: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(flatten)]
    inner: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    base_url: String,
}

/// `name` is a legal gateway id: `[a-z][a-z0-9_-]*`, not a built-in adapter.
pub fn validate_name(name: &str) -> Result<(), String> {
    let mut cs = name.chars();
    let Some(first) = cs.next() else {
        return Err("provider name is empty".into());
    };
    if !first.is_ascii_lowercase()
        || !cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        || name.len() > 32
    {
        return Err("provider name: 1–32 chars, start with a-z, then a-z, 0-9, _ or -".into());
    }
    // 85 §0.8: the built-in key providers are secrets, not gateways.
    if matches!(name, "opencode" | "openrouter" | "open_router" | "zen") {
        return Err(format!(
            "`{name}` is a built-in provider — set the `opencode` / `openrouter` secret instead"
        ));
    }
    // Which namespaces have a backend (12 §6); the only other place that knows is backend.rs.
    if name == "cursor"
        || name.starts_with("genai_")
        || oauth::provider(name).is_some()
        || AdapterKind::from_lower_str(name).is_some()
    {
        return Err(format!(
            "`{name}` is a built-in provider; pick another name"
        ));
    }
    Ok(())
}

/// Trim, require http(s), ensure a trailing `/` so genai's OpenAI adapter can
/// `Url::join` `chat/completions` and `models`.
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if !(raw.starts_with("http://") || raw.starts_with("https://")) {
        return Err("base url must be http(s)".into());
    }
    if raw.contains(char::is_whitespace) {
        return Err("base url must not contain spaces".into());
    }
    let rest = raw.split_once("://").map(|(_, r)| r).unwrap_or(raw);
    if rest.is_empty() || rest.starts_with('/') {
        return Err("base url needs a host".into());
    }
    let mut s = raw.to_owned();
    if !s.ends_with('/') {
        s.push('/');
    }
    Ok(s)
}

/// Gateways that have both a `providers.yaml` row and a key in the vault.
/// `vault: None` (the vault failed to open) yields no gateways.
pub(crate) fn load(data_dir: &Path, vault: Option<&secrets::Vault>) -> HashMap<String, Gateway> {
    let Some(vault) = vault else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for (name, url) in read_file(data_dir) {
        if let Some(key) = vault.get(&name, 0).filter(|k| !k.is_empty()) {
            out.insert(
                name,
                Gateway {
                    base_url: url,
                    key: key.to_string(),
                },
            );
        }
    }
    out
}

/// The running secrets actor, or why a gateway cannot be saved.
pub fn secrets_actor() -> Result<ActorRef<secrets::SecretsActor>, String> {
    ActorRef::<secrets::SecretsActor>::lookup(secrets::SECRETS_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "secrets vault unavailable".to_owned())
}

fn read_file(data_dir: &Path) -> BTreeMap<String, String> {
    let path = data_dir.join("providers.yaml");
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_yaml::from_str::<File>(&s)
            .map(|f| f.inner.into_iter().map(|(n, e)| (n, e.base_url)).collect())
            .unwrap_or_else(|e| {
                tracing::warn!("{}: {e}; ignoring", path.display());
                BTreeMap::new()
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => {
            tracing::warn!("{}: {e}; ignoring", path.display());
            BTreeMap::new()
        }
    }
}

fn write_file(data_dir: &Path, rows: &BTreeMap<String, String>) -> Result<(), String> {
    let path = data_dir.join("providers.yaml");
    let file = File {
        inner: rows
            .iter()
            .map(|(n, u)| {
                (
                    n.clone(),
                    Entry {
                        base_url: u.clone(),
                    },
                )
            })
            .collect(),
    };
    let text = serde_yaml::to_string(&file).map_err(|e| e.to_string())?;
    atomic_write(&path, text.as_bytes(), 0o644)
}

/// Merge `name`'s models into `<data_dir>/models.json` (user file; bundled is untouched).
pub fn write_models(data_dir: &Path, name: &str, ids: &[String]) -> Result<(), String> {
    let path = data_dir.join("models.json");
    let mut user: BTreeMap<String, Provider> = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e.to_string()),
    };
    let models = ids
        .iter()
        .map(|id| {
            (
                id.clone(),
                Model {
                    id: id.clone(),
                    name: id.clone(),
                    limit: Limit {
                        context: 0,
                        output: 0,
                    },
                    cost: None,
                    reasoning: false,
                    tool_call: true,
                    image: false,
                    reasoning_options: Vec::new(),
                    protocol: None,
                    canonical: None,
                },
            )
        })
        .collect();
    user.insert(
        name.to_owned(),
        Provider {
            id: name.to_owned(),
            name: name.to_owned(),
            env: vec![],
            models,
        },
    );
    let text = serde_json::to_string_pretty(&user).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| e.to_string())
}

fn remove_models(data_dir: &Path, name: &str) -> Result<(), String> {
    let path = data_dir.join("models.json");
    let Ok(s) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let mut user: BTreeMap<String, Provider> = serde_json::from_str(&s).unwrap_or_default();
    if user.remove(name).is_none() {
        return Ok(());
    }
    if user.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&user).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| e.to_string())
}

/// Persist a gateway: key into the vault (source `provider add`), url into
/// `providers.yaml`, optional catalog models. Overwrites the same name.
pub async fn save(
    data_dir: &Path,
    actor: &ActorRef<secrets::SecretsActor>,
    name: &str,
    base_url: &str,
    key: &str,
    models: &[String],
) -> Result<(), String> {
    validate_name(name)?;
    let base_url = normalize_url(base_url)?;
    if key.trim().is_empty() {
        return Err("api key is empty".into());
    }
    actor
        .ask(secrets::Set {
            name: name.to_owned(),
            value: key.trim().to_owned(),
            note: Some(format!("gateway {name} key")),
            source: "provider add".into(),
        })
        .await
        .map_err(|e| e.to_string())?;
    let mut rows = read_file(data_dir);
    rows.insert(name.to_owned(), base_url);
    write_file(data_dir, &rows)?;
    write_models(data_dir, name, models)
}

/// Drop a gateway's url, key, and catalog models. A key already gone from
/// the vault (the user deleted it) is not an error.
pub async fn remove(
    data_dir: &Path,
    actor: &ActorRef<secrets::SecretsActor>,
    name: &str,
) -> Result<(), String> {
    let mut rows = read_file(data_dir);
    if rows.remove(name).is_none() {
        return Err(format!("no provider `{name}`"));
    }
    write_file(data_dir, &rows)?;
    if let Err(e) = actor
        .ask(secrets::Delete {
            name: name.to_owned(),
        })
        .await
    {
        tracing::warn!(provider = name, "removing key from the vault: {e}");
    }
    remove_models(data_dir, name)
}

fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(mode);
    }
    // No POSIX mode bits off unix; the per-user profile directory carries the ACL.
    #[cfg(not(unix))]
    let _ = mode;
    open.open(&tmp)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_url_rules() {
        assert!(validate_name("rwx").is_ok());
        assert!(validate_name("my-gw_1").is_ok());
        assert!(validate_name("openai").is_err());
        assert!(validate_name("cursor").is_err());
        assert!(validate_name("Anthropic").is_err());
        assert!(validate_name("1rwx").is_err());
        // 85 §0.8: the built-ins are secrets, not gateways; the error points there.
        for name in ["opencode", "openrouter", "open_router", "zen"] {
            let err = validate_name(name).unwrap_err();
            assert!(
                err.contains("set the `opencode` / `openrouter` secret"),
                "{name}: {err}"
            );
        }
        assert_eq!(
            normalize_url("https://api.ai.rwx.dev/v1").unwrap(),
            "https://api.ai.rwx.dev/v1/"
        );
        assert!(normalize_url("ftp://x").is_err());
        assert!(normalize_url("not a url").is_err());
    }

    #[tokio::test]
    async fn save_roundtrip_and_remove() {
        use kameo::actor::Spawn;
        use std::sync::Arc;
        let scratch = crate::TestDir::new("prov");
        let dir = scratch.0.clone();
        let vault = Arc::new(
            secrets::Vault::open(&dir, secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let actor = secrets::SecretsActor::spawn(secrets::SecretsActor::new(
            vault.clone(),
            rt,
            std::time::Duration::from_secs(1),
        ));
        save(
            &dir,
            &actor,
            "rwx",
            "https://api.ai.rwx.dev/v1",
            "sk-test",
            &["m1".into()],
        )
        .await
        .unwrap();
        assert_eq!(vault.get("rwx", 0).unwrap().as_str(), "sk-test");
        let g = load(&dir, Some(&vault));
        assert_eq!(g["rwx"].base_url, "https://api.ai.rwx.dev/v1/");
        assert_eq!(g["rwx"].key, "sk-test");
        let models: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("models.json")).unwrap())
                .unwrap();
        assert_eq!(models["rwx"]["models"]["m1"]["id"], "m1");
        remove(&dir, &actor, "rwx").await.unwrap();
        assert!(vault.get("rwx", 0).is_none());
        assert!(load(&dir, Some(&vault)).is_empty());
        assert!(load(&dir, None).is_empty());
    }
}
