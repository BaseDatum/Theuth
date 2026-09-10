//! Named OpenAI-compatible gateways (`<data_dir>/providers.yaml`) plus their
//! keys in `credentials.yaml`. User-facing ids are `name/model`; genai sees
//! the OpenAI adapter with a rewritten endpoint.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;

use genai::adapter::AdapterKind;
use models::{Limit, Model, Provider};
use serde::{Deserialize, Serialize};

/// One named OpenAI-compatible gateway.
#[derive(Clone, Debug)]
pub(crate) struct Gateway {
    /// Base URL, trailing slash (`https://api.example/v1/`).
    pub base_url: String,
    /// API key from `credentials.yaml`.
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

/// Gateways that have both a `providers.yaml` row and a key.
pub(crate) fn load(data_dir: &Path, creds: &HashMap<String, String>) -> HashMap<String, Gateway> {
    let mut out = HashMap::new();
    for (name, url) in read_file(data_dir) {
        if let Some(key) = creds.get(&name).filter(|k| !k.is_empty()) {
            out.insert(
                name,
                Gateway {
                    base_url: url,
                    key: key.clone(),
                },
            );
        }
    }
    out
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

/// Insert or replace `name: value` in a flat YAML map file (credentials.yaml).
/// Comments and unknown lines stay; the matching key line is rewritten.
pub fn upsert_credential(data_dir: &Path, name: &str, value: &str) -> Result<(), String> {
    let path = data_dir.join("credentials.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let prefix = format!("{name}:");
    let rendered = match serde_yaml::to_string(&HashMap::from([(name, value)])) {
        Ok(s) => s,
        Err(e) => return Err(e.to_string()),
    };
    let line = rendered.trim_end().to_owned();
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    match lines.iter().position(|l| {
        let t = l.trim_start();
        !t.starts_with('#') && t.starts_with(&prefix)
    }) {
        Some(i) => lines[i] = line,
        None => {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.is_empty()) {
                lines.push(String::new());
            }
            lines.push(line);
        }
    }
    atomic_write(&path, (lines.join("\n") + "\n").as_bytes(), 0o600)
}

fn remove_credential(data_dir: &Path, name: &str) -> Result<(), String> {
    let path = data_dir.join("credentials.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let prefix = format!("{name}:");
    let lines: Vec<String> = text
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with('#') || !t.starts_with(&prefix)
        })
        .map(str::to_owned)
        .collect();
    atomic_write(&path, (lines.join("\n") + "\n").as_bytes(), 0o600)
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

/// Persist a gateway (url + key + optional catalog models). Overwrites the same name.
pub fn save(
    data_dir: &Path,
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
    let mut rows = read_file(data_dir);
    rows.insert(name.to_owned(), base_url);
    write_file(data_dir, &rows)?;
    upsert_credential(data_dir, name, key.trim())?;
    write_models(data_dir, name, models)
}

/// Drop a gateway's url, key, and catalog models.
pub fn remove(data_dir: &Path, name: &str) -> Result<(), String> {
    let mut rows = read_file(data_dir);
    if rows.remove(name).is_none() {
        return Err(format!("no provider `{name}`"));
    }
    write_file(data_dir, &rows)?;
    remove_credential(data_dir, name)?;
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
        assert_eq!(
            normalize_url("https://api.ai.rwx.dev/v1").unwrap(),
            "https://api.ai.rwx.dev/v1/"
        );
        assert!(normalize_url("ftp://x").is_err());
        assert!(normalize_url("not a url").is_err());
    }

    #[test]
    fn save_roundtrip_and_remove() {
        let dir = std::env::temp_dir().join(format!("wth-prov-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        save(
            &dir,
            "rwx",
            "https://api.ai.rwx.dev/v1",
            "sk-test",
            &["m1".into()],
        )
        .unwrap();
        let creds = HashMap::from([("rwx".into(), "sk-test".into())]);
        let g = load(&dir, &creds);
        assert_eq!(g["rwx"].base_url, "https://api.ai.rwx.dev/v1/");
        assert_eq!(g["rwx"].key, "sk-test");
        let models: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("models.json")).unwrap())
                .unwrap();
        assert_eq!(models["rwx"]["models"]["m1"]["id"], "m1");
        remove(&dir, "rwx").unwrap();
        assert!(load(&dir, &creds).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn credential_upsert_keeps_comments() {
        let dir = std::env::temp_dir().join(format!("wth-cred-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("credentials.yaml"),
            "# keep me\n# openai: sk-...\nanthropic: sk-ant-1\n",
        )
        .unwrap();
        upsert_credential(&dir, "rwx", "sk-new").unwrap();
        upsert_credential(&dir, "anthropic", "sk-ant-2").unwrap();
        let text = std::fs::read_to_string(dir.join("credentials.yaml")).unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("# openai: sk-..."));
        assert!(text.contains("anthropic: sk-ant-2"));
        assert!(text.contains("rwx: sk-new"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
