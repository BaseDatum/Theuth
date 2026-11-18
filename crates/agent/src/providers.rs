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

use crate::LocalRuntime;

/// One custom provider (named OpenAI-compatible endpoint).
#[derive(Clone, Debug, Default)]
pub(crate) struct Gateway {
    /// Base URL, trailing slash (`https://api.example/v1/`).
    pub base_url: String,
    /// API key; [`crate::supervisor::Auth::gateway`] replaces it with the live vault value.
    /// Empty: a keyless local provider (177 E4) — model calls carry the
    /// placeholder key [`crate::backend::KEYLESS_PLACEHOLDER`].
    pub key: String,
    /// 177: the runtime preset it was added with.
    pub runtime: Option<LocalRuntime>,
}

impl Gateway {
    /// 177: a runtime preset, or a private / loopback host ([`crate::local::private_url`]).
    pub fn local(&self) -> bool {
        self.runtime.is_some() || crate::local::private_url(&self.base_url)
    }

    /// 177 R2-5: the narrow rule for behaviour — a runtime preset, or a
    /// loopback host ([`crate::local::loopback_url`]). Thinking-off kwargs and
    /// the assumed window follow it; a LAN gateway in front of cloud models
    /// (LiteLLM on 10.x) is [`Gateway::local`] but not this.
    pub fn on_this_machine(&self) -> bool {
        self.runtime.is_some() || crate::local::loopback_url(&self.base_url)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(flatten)]
    inner: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Entry {
    base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime: Option<LocalRuntime>,
    /// R2-3: this provider's add stored the vault secret of its name (`false`:
    /// keyless). Absent on rows written before 177, whose adds always stored one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owns_key: Option<bool>,
}

impl Entry {
    fn local(&self) -> bool {
        self.runtime.is_some() || crate::local::private_url(&self.base_url)
    }

    /// The vault secret of the provider's name is the provider's own.
    fn owns_key(&self) -> bool {
        self.owns_key != Some(false)
    }
}

/// Runtime names a LOCAL provider may take, shadowing genai's adapter of that name (177 E4).
const RUNTIME_NAMES: [&str; 5] = ["ollama", "omlx", "lmstudio", "llamacpp", "vllm"];

/// `name` is a legal gateway id: `[a-z][a-z0-9_-]*`, not a built-in adapter —
/// except that a `local` provider may take a runtime's own name (177 E4).
pub fn validate_name(name: &str, local: bool) -> Result<(), String> {
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
    if local && RUNTIME_NAMES.contains(&name) {
        return Ok(());
    }
    // 85 §0.8: the built-in key providers are secrets, not gateways.
    if matches!(
        name,
        "opencode" | "opencode-go" | "openrouter" | "open_router" | "zen"
    ) {
        return Err(format!(
            "`{name}` is a built-in provider — set the `opencode` / `opencode-go` / `openrouter` secret instead"
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

/// Gateways with a `providers.yaml` row and a key in the vault, plus local rows
/// with no key (keyless, 177 E4). `vault: None` (the vault failed to open)
/// yields only the keyless local ones.
pub(crate) fn load(data_dir: &Path, vault: Option<&secrets::Vault>) -> HashMap<String, Gateway> {
    let mut out = HashMap::new();
    for (name, entry) in read_file(data_dir) {
        let key = vault
            .and_then(|v| v.get(&name, 0))
            .filter(|k| !k.is_empty())
            .map(|k| k.to_string());
        if key.is_none() && !entry.local() {
            continue;
        }
        out.insert(
            name,
            Gateway {
                base_url: entry.base_url,
                key: key.unwrap_or_default(),
                runtime: entry.runtime,
            },
        );
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

fn read_file(data_dir: &Path) -> BTreeMap<String, Entry> {
    let path = data_dir.join("providers.yaml");
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_yaml::from_str::<File>(&s)
            .map(|f| f.inner)
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

fn write_file(data_dir: &Path, rows: BTreeMap<String, Entry>) -> Result<(), String> {
    let path = data_dir.join("providers.yaml");
    let text = serde_yaml::to_string(&File { inner: rows }).map_err(|e| e.to_string())?;
    atomic_write(&path, text.as_bytes(), 0o644)
}

/// Which ids a listing may change in `models.json[name]` (162 D4).
#[derive(Clone, Copy)]
pub enum Listing<'a> {
    /// A custom provider: the listing is the truth for which ids exist.
    Custom {
        /// 177 E7: a local model server (its block is marked; no sibling fill).
        local: bool,
        /// 177 E5: context windows the server reported, by id.
        contexts: &'a HashMap<String, u32>,
    },
    /// A built-in: add only ids absent from `catalog` (bundled plus user file); never remove.
    Builtin(&'a models::Catalog),
}

/// Merge a model listing into `<data_dir>/models.json` (the user file; bundled is
/// untouched) and return the ids added and removed, each sorted (162 D4):
/// - custom: a still-listed id keeps its entry as is (hand edits included), a new id
///   gets a context-0 entry (it inherits from its models.dev sibling unless the
///   provider is local), an unlisted id goes; a server-reported context (177 E5)
///   fills an unknown or server-set window, never a user one;
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
    let mut user = read_models(data_dir)?;
    let entry = user.entry(name.to_owned()).or_insert_with(|| Provider {
        id: name.to_owned(),
        name: match listing {
            Listing::Custom { .. } => name.to_owned(),
            Listing::Builtin(_) => String::new(),
        },
        env: vec![],
        local: false,
        models: BTreeMap::new(),
    });
    let listed: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let mut removed = Vec::new();
    let mut changed = false;
    if let Listing::Custom { local, contexts } = listing {
        changed = entry.local != local;
        entry.local = local;
        entry.models.retain(|id, _| {
            let keep = listed.contains(id.as_str());
            if !keep {
                removed.push(id.clone());
            }
            keep
        });
        for (id, m) in &mut entry.models {
            let l = &mut m.limit;
            let server_owned = l.source == models::LimitSource::Server
                || (l.source == models::LimitSource::Unknown && l.context == 0);
            if let Some(&c) = contexts.get(id)
                && c > 0
                && server_owned
                && (l.context, l.source) != (c, models::LimitSource::Server)
            {
                l.context = c;
                l.source = models::LimitSource::Server;
                changed = true;
            }
        }
    }
    let mut added = Vec::new();
    for id in listed {
        let known = entry.models.contains_key(id)
            || matches!(listing, Listing::Builtin(c) if c.get(&format!("{name}/{id}")).is_some());
        if !known {
            let context = match listing {
                Listing::Custom { contexts, .. } => contexts.get(id).copied().unwrap_or(0),
                Listing::Builtin(_) => 0,
            };
            entry
                .models
                .insert(id.to_owned(), listed_model(id, context));
            added.push(id.to_owned());
        }
    }
    if added.is_empty() && removed.is_empty() && !changed {
        return Ok((added, removed));
    }
    write_user_models(data_dir, &user)?;
    Ok((added, removed))
}

/// The user `models.json` (missing = empty); a broken file is an error, never
/// overwritten with just one provider.
pub(crate) fn read_models(data_dir: &Path) -> Result<BTreeMap<String, Provider>, String> {
    let path = data_dir.join("models.json");
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// Atomically replace the user `models.json`.
pub(crate) fn write_user_models(
    data_dir: &Path,
    user: &BTreeMap<String, Provider>,
) -> Result<(), String> {
    let text = serde_json::to_string_pretty(user).map_err(|e| e.to_string())?;
    atomic_write(
        &data_dir.join("models.json"),
        (text + "\n").as_bytes(),
        0o644,
    )
}

/// A listed id with no metadata but a server-reported `context` (0 = unknown:
/// the catalog fills it from a sibling unless the provider is local).
fn listed_model(id: &str, context: u32) -> Model {
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        limit: Limit {
            context,
            output: 0,
            source: if context > 0 {
                models::LimitSource::Server
            } else {
                models::LimitSource::Unknown
            },
            assumed: 0,
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

/// 177 E6: set (`Some`) or clear (`None`) a model's limits in the user
/// `models.json`, source `user` (unknown when both end up 0); the model must
/// already be listed there or in the bundled catalog. Returns the stored model.
pub fn set_limits(
    data_dir: &Path,
    catalog: &models::Catalog,
    provider: &str,
    model: &str,
    context: Option<u32>,
    output: Option<u32>,
) -> Result<Model, String> {
    let mut user = read_models(data_dir)?;
    let base = user
        .get(provider)
        .and_then(|p| p.models.get(model).cloned())
        .or_else(|| {
            catalog
                .get(&format!("{provider}/{model}"))
                .map(|(_, m)| m.clone())
        })
        .ok_or_else(|| format!("no model {provider}/{model}"))?;
    let block = user.entry(provider.to_owned()).or_insert_with(|| Provider {
        id: provider.to_owned(),
        name: String::new(),
        env: vec![],
        local: false,
        models: BTreeMap::new(),
    });
    let mut m = base;
    m.limit.context = context.unwrap_or(0);
    m.limit.output = output.unwrap_or(0);
    m.limit.source = if m.limit.context == 0 && m.limit.output == 0 {
        models::LimitSource::Unknown
    } else {
        models::LimitSource::User
    };
    block.models.insert(model.to_owned(), m.clone());
    write_user_models(data_dir, &user)?;
    Ok(m)
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

/// Persist a custom provider: key into the vault (source `provider add`), url and
/// runtime into `providers.yaml`. Overwrites the same name; its models are
/// [`write_models`]'. An empty key is allowed only for a local provider (177 E4).
///
/// R2-3: the row records whether this provider wrote the vault secret of its
/// name (`owns_key`). A keyless add deletes that secret only when it replaces
/// a row that owned it — a first keyless `omlx` keeps the user's `omlx` key.
pub async fn save(
    data_dir: &Path,
    actor: &ActorRef<secrets::SecretsActor>,
    name: &str,
    base_url: &str,
    key: &str,
    runtime: Option<LocalRuntime>,
) -> Result<(), String> {
    let key = key.trim();
    let entry = Entry {
        base_url: normalize_url(base_url)?,
        runtime,
        owns_key: Some(!key.is_empty()),
    };
    validate_name(name, entry.local())?;
    let mut rows = read_file(data_dir);
    if key.is_empty() {
        if !entry.local() {
            return Err("api key is empty".into());
        }
        // A keyless re-add must not keep sending the key this provider stored.
        if rows.get(name).is_some_and(Entry::owns_key) {
            let _ = actor
                .ask(secrets::Delete {
                    name: name.to_owned(),
                })
                .await;
        }
    } else {
        actor
            .ask(secrets::Set {
                name: name.to_owned(),
                value: key.to_owned(),
                note: Some(format!("custom provider {name} key")),
                source: "provider add".into(),
            })
            .await
            .map_err(|e| e.to_string())?;
    }
    rows.insert(name.to_owned(), entry);
    write_file(data_dir, rows)
}

/// Drop a custom provider's url, its key (only one its add stored, R2-3), and
/// its catalog models. A key already gone from the vault is not an error.
pub async fn remove(
    data_dir: &Path,
    actor: &ActorRef<secrets::SecretsActor>,
    name: &str,
) -> Result<(), String> {
    let mut rows = read_file(data_dir);
    let Some(entry) = rows.remove(name) else {
        return Err(format!("no provider `{name}`"));
    };
    write_file(data_dir, rows)?;
    if entry.owns_key()
        && let Err(e) = actor
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

    static NONE: std::sync::LazyLock<HashMap<String, u32>> = std::sync::LazyLock::new(HashMap::new);

    /// A cloud custom provider's listing with no server-reported windows.
    fn cloud() -> Listing<'static> {
        Listing::Custom {
            local: false,
            contexts: &NONE,
        }
    }

    #[test]
    fn name_and_url_rules() {
        assert!(validate_name("rwx", false).is_ok());
        assert!(validate_name("my-gw_1", false).is_ok());
        assert!(validate_name("openai", false).is_err());
        assert!(validate_name("cursor", false).is_err());
        assert!(validate_name("Anthropic", false).is_err());
        assert!(validate_name("1rwx", false).is_err());
        // 177 E4: a runtime's own name is a local provider's only.
        for name in ["omlx", "ollama", "lmstudio", "llamacpp", "vllm"] {
            assert!(validate_name(name, true).is_ok(), "{name}");
        }
        assert_eq!(
            validate_name("omlx", false).unwrap_err(),
            "`omlx` is a built-in provider; pick another name"
        );
        assert!(validate_name("openai", true).is_err());
        // 85 §0.8: the built-ins are secrets, not gateways; the error points there.
        for name in [
            "opencode",
            "opencode-go",
            "openrouter",
            "open_router",
            "zen",
        ] {
            let err = validate_name(name, false).unwrap_err();
            assert!(
                err.contains("set the `opencode` / `opencode-go` / `openrouter` secret"),
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
            None,
        )
        .await
        .unwrap();
        write_models(&dir, "rwx", &["m1".into()], cloud()).unwrap();
        assert_eq!(vault.get("rwx", 0).unwrap().as_str(), "sk-test");
        let g = load(&dir, Some(&vault));
        assert_eq!(g["rwx"].base_url, "https://api.ai.rwx.dev/v1/");
        assert_eq!(g["rwx"].key, "sk-test");
        // A hand edit survives a re-add (save + the same listing): 162 D4.
        hand_edit(&dir, "rwx", "m1");
        save(
            &dir,
            &actor,
            "rwx",
            "https://api.ai.rwx.dev/v1",
            "sk-two",
            None,
        )
        .await
        .unwrap();
        let before = std::fs::read(dir.join("models.json")).unwrap();
        assert_eq!(
            write_models(&dir, "rwx", &["m1".into()], cloud()).unwrap(),
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

    /// R2-3: a keyless add and its remove never touch a secret the provider
    /// did not store (the user's genai `omlx` key); a provider that stored its
    /// key and is re-added keyless drops it; a pre-177 row owns its key.
    #[tokio::test]
    async fn keyless_add_and_remove_keep_foreign_secrets() {
        use kameo::actor::Spawn;
        use std::sync::Arc;
        let scratch = crate::TestDir::new("prov-r23");
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
        let key = |n: &str| vault.get(n, 0).map(|k| k.as_str().to_owned());
        vault
            .set("omlx", "user-genai-key", None, "test")
            .await
            .unwrap();
        save(&dir, &actor, "omlx", "http://127.0.0.1:8000/v1", "", None)
            .await
            .unwrap();
        assert_eq!(key("omlx").as_deref(), Some("user-genai-key"));
        remove(&dir, &actor, "omlx").await.unwrap();
        assert_eq!(key("omlx").as_deref(), Some("user-genai-key"));

        save(
            &dir,
            &actor,
            "lms",
            "http://127.0.0.1:1234/v1",
            "sk-lms",
            None,
        )
        .await
        .unwrap();
        assert_eq!(key("lms").as_deref(), Some("sk-lms"));
        save(&dir, &actor, "lms", "http://127.0.0.1:1234/v1", "", None)
            .await
            .unwrap();
        assert_eq!(
            key("lms"),
            None,
            "a keyless re-add drops the key it replaced"
        );

        // A row written before 177 (no `owns_key`): its add stored the key.
        vault
            .set("mlx", "sk-mlx", None, "provider add")
            .await
            .unwrap();
        std::fs::write(
            dir.join("providers.yaml"),
            "mlx:\n  base_url: http://127.0.0.1:8080/v1/\n",
        )
        .unwrap();
        assert_eq!(load(&dir, Some(&vault))["mlx"].key, "sk-mlx");
        remove(&dir, &actor, "mlx").await.unwrap();
        assert_eq!(key("mlx"), None);
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

    /// 177 E5: a server-reported window fills an unknown entry and follows the
    /// server later; a hand edit (no source, a window) is never overwritten;
    /// a local listing marks the block.
    #[test]
    fn server_windows_fill_unknown_never_user() {
        let scratch = crate::TestDir::new("prov-windows");
        let dir = scratch.0.clone();
        let ids = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let local = |c: &[(&str, u32)]| -> HashMap<String, u32> {
            c.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
        };
        let listing = |contexts| Listing::Custom {
            local: true,
            contexts,
        };
        let first = local(&[("a", 8192)]);
        write_models(&dir, "lm", &ids(&["a", "b"]), listing(&first)).unwrap();
        let v = user_file(&dir);
        assert_eq!(v["lm"]["local"], true);
        assert_eq!(
            v["lm"]["models"]["a"]["limit"],
            serde_json::json!({"context": 8192, "output": 0, "source": "server"})
        );
        assert!(v["lm"]["models"]["b"]["limit"].get("source").is_none());
        hand_edit(&dir, "lm", "b");
        let later = local(&[("a", 4096), ("b", 999)]);
        write_models(&dir, "lm", &ids(&["a", "b"]), listing(&later)).unwrap();
        let v = user_file(&dir);
        assert_eq!(v["lm"]["models"]["a"]["limit"]["context"], 4096);
        assert_eq!(v["lm"]["models"]["b"]["limit"]["context"], 1234);
    }

    /// 162 D4 custom: the listing is the truth; listed entries are kept as they
    /// are (a hand-edited `protocol` included), new ids are context 0, gone ids go.
    #[test]
    fn custom_listing_keeps_adds_and_removes() {
        let scratch = crate::TestDir::new("prov-custom");
        let dir = scratch.0.clone();
        let ids = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            write_models(&dir, "rwx", &ids(&["b", "a"]), cloud()).unwrap(),
            (ids(&["a", "b"]), vec![])
        );
        assert_eq!(user_file(&dir)["rwx"]["name"], "rwx");
        hand_edit(&dir, "rwx", "a");
        assert_eq!(
            write_models(&dir, "rwx", &ids(&["c", "a"]), cloud()).unwrap(),
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
        write_models(&dir, "rwx", &["a".into()], cloud()).unwrap();
        let before = std::fs::read(dir.join("models.json")).unwrap();
        let e = write_models(&dir, "rwx", &[], cloud()).unwrap_err();
        assert_eq!(e, "rwx listed no models");
        assert_eq!(std::fs::read(dir.join("models.json")).unwrap(), before);
        std::fs::write(dir.join("models.json"), "{ not json").unwrap();
        assert!(write_models(&dir, "rwx", &["b".into()], cloud()).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("models.json")).unwrap(),
            "{ not json"
        );
    }
}
