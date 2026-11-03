//! Custom providers: named OpenAI-compatible endpoints (`<data_dir>/providers.yaml`)
//! plus their keys in the secrets vault (37 §3.6), under the provider's name. User-facing
//! ids are `name/model`; genai sees the OpenAI adapter with a rewritten endpoint.
//! Internally (and in the persisted ids) they are still called gateways (162 D1).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;

use genai::adapter::AdapterKind;
use kameo::actor::ActorRef;
use models::{Limit, Model, Provider};
use serde::{Deserialize, Serialize};

/// One custom provider (named OpenAI-compatible endpoint).
#[derive(Clone, Debug)]
pub(crate) struct Gateway {
    /// Base URL, trailing slash (`https://api.example/v1/`).
    pub base_url: String,
    /// API key; [`crate::supervisor::Auth::gateway`] replaces it with the live vault value.
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

/// Which ids a listing may change in `models.json[name]` (162 D4).
#[derive(Clone, Copy)]
pub enum Listing<'a> {
    /// A custom provider: the listing is the truth for which ids exist.
    Custom,
    /// A built-in: add only ids absent from `catalog` (bundled plus user file); never remove.
    Builtin(&'a models::Catalog),
}

/// Merge a model listing into `<data_dir>/models.json` (the user file; bundled is
/// untouched) and return the ids added and removed, each sorted (162 D4):
/// - custom: a still-listed id keeps its entry as is (hand edits included), a new id
///   gets a context-0 entry (it inherits from its models.dev sibling), an unlisted id goes;
/// - built-in: only ids neither the catalog nor the user file has are added, under a
///   block with no `name`/`env`, so the bundled provider metadata stays.
///
/// An empty listing is an error and the file is not touched; no change writes nothing.
pub fn write_models(
    data_dir: &Path,
    name: &str,
    ids: &[String],
    listing: Listing<'_>,
) -> Result<(Vec<String>, Vec<String>), String> {
    if ids.is_empty() {
        return Err(format!("{name} listed no models"));
    }
    let path = data_dir.join("models.json");
    // A broken user file is an error, never overwritten with just this provider.
    let mut user: BTreeMap<String, Provider> = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e.to_string()),
    };
    let entry = user.entry(name.to_owned()).or_insert_with(|| Provider {
        id: name.to_owned(),
        name: match listing {
            Listing::Custom => name.to_owned(),
            Listing::Builtin(_) => String::new(),
        },
        env: vec![],
        models: BTreeMap::new(),
    });
    let listed: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let mut removed = Vec::new();
    if let Listing::Custom = listing {
        entry.models.retain(|id, _| {
            let keep = listed.contains(id.as_str());
            if !keep {
                removed.push(id.clone());
            }
            keep
        });
    }
    let mut added = Vec::new();
    for id in listed {
        let known = entry.models.contains_key(id)
            || matches!(listing, Listing::Builtin(c) if c.get(&format!("{name}/{id}")).is_some());
        if !known {
            entry.models.insert(id.to_owned(), listed_model(id));
            added.push(id.to_owned());
        }
    }
    if added.is_empty() && removed.is_empty() {
        return Ok((added, removed));
    }
    let text = serde_json::to_string_pretty(&user).map_err(|e| e.to_string())?;
    atomic_write(&path, (text + "\n").as_bytes(), 0o644)?;
    Ok((added, removed))
}

/// A listed id with no metadata: context 0 so the catalog fills it from a sibling.
fn listed_model(id: &str) -> Model {
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
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
    }
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

/// Persist a custom provider: key into the vault (source `provider add`), url into
/// `providers.yaml`. Overwrites the same name; its models are [`write_models`]'.
pub async fn save(
    data_dir: &Path,
    actor: &ActorRef<secrets::SecretsActor>,
    name: &str,
    base_url: &str,
    key: &str,
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
            note: Some(format!("custom provider {name} key")),
            source: "provider add".into(),
        })
        .await
        .map_err(|e| e.to_string())?;
    let mut rows = read_file(data_dir);
    rows.insert(name.to_owned(), base_url);
    write_file(data_dir, &rows)
}

/// Drop a custom provider's url, key, and catalog models. A key already gone from
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
        save(&dir, &actor, "rwx", "https://api.ai.rwx.dev/v1", "sk-test")
            .await
            .unwrap();
        write_models(&dir, "rwx", &["m1".into()], Listing::Custom).unwrap();
        assert_eq!(vault.get("rwx", 0).unwrap().as_str(), "sk-test");
        let g = load(&dir, Some(&vault));
        assert_eq!(g["rwx"].base_url, "https://api.ai.rwx.dev/v1/");
        assert_eq!(g["rwx"].key, "sk-test");
        // A hand edit survives a re-add (save + the same listing): 162 D4.
        hand_edit(&dir, "rwx", "m1");
        save(&dir, &actor, "rwx", "https://api.ai.rwx.dev/v1", "sk-two")
            .await
            .unwrap();
        let before = std::fs::read(dir.join("models.json")).unwrap();
        assert_eq!(
            write_models(&dir, "rwx", &["m1".into()], Listing::Custom).unwrap(),
            (vec![], vec![])
        );
        assert_eq!(std::fs::read(dir.join("models.json")).unwrap(), before);
        assert_eq!(vault.get("rwx", 0).unwrap().as_str(), "sk-two");
        remove(&dir, &actor, "rwx").await.unwrap();
        assert!(vault.get("rwx", 0).is_none());
        assert!(load(&dir, Some(&vault)).is_empty());
        assert!(load(&dir, None).is_empty());
        assert!(!dir.join("models.json").exists());
    }

    fn user_file(dir: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("models.json")).unwrap()).unwrap()
    }

    /// Give `provider/id` a hand-written protocol and context, as a user would.
    fn hand_edit(dir: &Path, provider: &str, id: &str) {
        let mut v = user_file(dir);
        v[provider]["models"][id]["protocol"] = "anthropic".into();
        v[provider]["models"][id]["limit"]["context"] = 1234.into();
        std::fs::write(dir.join("models.json"), v.to_string()).unwrap();
    }

    /// 162 D4 custom: the listing is the truth; listed entries are kept as they
    /// are (a hand-edited `protocol` included), new ids are context 0, gone ids go.
    #[test]
    fn custom_listing_keeps_adds_and_removes() {
        let scratch = crate::TestDir::new("prov-custom");
        let dir = scratch.0.clone();
        let ids = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            write_models(&dir, "rwx", &ids(&["b", "a"]), Listing::Custom).unwrap(),
            (ids(&["a", "b"]), vec![])
        );
        assert_eq!(user_file(&dir)["rwx"]["name"], "rwx");
        hand_edit(&dir, "rwx", "a");
        assert_eq!(
            write_models(&dir, "rwx", &ids(&["c", "a"]), Listing::Custom).unwrap(),
            (ids(&["c"]), ids(&["b"]))
        );
        let v = user_file(&dir);
        let models = v["rwx"]["models"].as_object().unwrap();
        assert_eq!(models.keys().collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(models["a"]["protocol"], "anthropic");
        assert_eq!(models["a"]["limit"]["context"], 1234);
        assert_eq!(models["c"]["limit"]["context"], 0);
        let cat = models::Catalog::load(&dir);
        assert_eq!(
            cat.get("rwx/a").unwrap().1.protocol,
            Some(models::Protocol::Anthropic)
        );
    }

    /// 162 D4 built-in: only ids neither bundled nor in the user file are added,
    /// nothing is removed, and the bundled provider metadata stays.
    #[test]
    fn builtin_listing_only_adds_unknown_ids() {
        let scratch = crate::TestDir::new("prov-builtin");
        let dir = scratch.0.clone();
        let cat = models::Catalog::load(&dir);
        let zen = cat
            .providers()
            .find(|p| p.id == "opencode")
            .unwrap()
            .clone();
        let bundled = zen.models.keys().next().unwrap().clone();
        let listing = vec![bundled.clone(), "zz-new".to_owned()];
        assert_eq!(
            write_models(&dir, "opencode", &listing, Listing::Builtin(&cat)).unwrap(),
            (vec!["zz-new".to_owned()], vec![])
        );
        let v = user_file(&dir);
        assert_eq!(
            v["opencode"]["models"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["zz-new"]
        );
        assert!(v["opencode"].get("name").is_none(), "{v}");
        // The user-file id is known now; a listing without it removes nothing.
        hand_edit(&dir, "opencode", "zz-new");
        let cat2 = models::Catalog::load(&dir);
        let before = std::fs::read(dir.join("models.json")).unwrap();
        assert_eq!(
            write_models(
                &dir,
                "opencode",
                std::slice::from_ref(&bundled),
                Listing::Builtin(&cat2)
            )
            .unwrap(),
            (vec![], vec![])
        );
        assert_eq!(std::fs::read(dir.join("models.json")).unwrap(), before);
        let p = cat2.providers().find(|p| p.id == "opencode").unwrap();
        assert_eq!((&p.name, &p.env), (&zen.name, &zen.env));
        assert_eq!(p.models.len(), zen.models.len() + 1);
        assert_eq!(
            cat2.get(&format!("opencode/{bundled}")).unwrap().1.protocol,
            zen.models[&bundled].protocol
        );
    }

    /// 162 D4: an empty listing or an unreadable user file changes nothing.
    #[test]
    fn empty_listing_or_broken_file_leaves_models_json_alone() {
        let scratch = crate::TestDir::new("prov-empty");
        let dir = scratch.0.clone();
        write_models(&dir, "rwx", &["a".into()], Listing::Custom).unwrap();
        let before = std::fs::read(dir.join("models.json")).unwrap();
        let e = write_models(&dir, "rwx", &[], Listing::Custom).unwrap_err();
        assert_eq!(e, "rwx listed no models");
        assert_eq!(std::fs::read(dir.join("models.json")).unwrap(), before);
        std::fs::write(dir.join("models.json"), "{ not json").unwrap();
        assert!(write_models(&dir, "rwx", &["b".into()], Listing::Custom).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("models.json")).unwrap(),
            "{ not json"
        );
    }
}
